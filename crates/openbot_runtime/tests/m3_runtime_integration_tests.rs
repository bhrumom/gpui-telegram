//! Comprehensive M3 Core Runtime Integration Tests (`OB-F06`..`OB-F10`).
//!
//! Exercises the full `OpenBotHostRuntime` stack end-to-end using RAII `tempfile::TempDir`
//! isolation (zero `/tmp` hardcoded paths).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write as _;

use openbot_contracts::agents::{
    AgentAccess, AgentBackupModels, AgentModelChoice, UpdateAgentInput,
};
use openbot_contracts::channels::{
    CHANNEL_ASSIGNMENT_LIMIT, CHANNEL_PARALLEL_LIMIT, CHANNEL_REPLY_LIMIT, ChannelMember,
    ChannelTaskState,
};
use openbot_contracts::conversation::{ConversationMessage, ConversationReactionActor};
use openbot_contracts::providers::{
    AgentProviderId, CustomProviderEndpoint, CustomProviderHarness, CustomProviderModel,
};
use openbot_contracts::queue::QueueEditRequest;
use openbot_contracts::ui_blocks::{UiBlockResponse, UiBlockSpec};
use openbot_runtime::{
    ApprovalDecision, ApprovalKind, ApprovalRequestInput, AskUiInput, BrowserRunStepsRequest,
    BrowserTarget, CHANNEL_TOOLS_COUNT, ChannelAssignInput, ChannelCancelInput,
    ChannelContinueInput, ChannelHistoryPacketInput, ChannelReassignInput, ChannelReplyInput,
    ChannelResultInput, ChannelTransferInput, CompactionReason, CreateAgentRequest,
    CreateChannelInput, DIRECT_BROWSER_COUNT, DIRECT_OPENBOT_COUNT, FULL_CATALOG_COUNT,
    FillCardRequest, FillFieldEntry, FillFieldsRequest, GenerateProfileRequest, HANDOFF_MAX_CHARS,
    HANDOFF_START, InterruptAgentInput, LinkPaymentRequest, MAX_BROWSER_TABS, MailboxSendInput,
    MemoryPutInput, MemorySearchInput, OpenBotHostRuntime, OpenBotRuntimeError, PinActor,
    PinMessageInput, PostChannelMessageInput, QueueAddInput, QueueSteerInput, ReactMessageInput,
    RegisterMcpServerInput, SaveSkillInput, SendChatMessageInput, SideQuestionInput,
    StartTurnRequest, SubmitSecretRequest, TakeoverRequest, ToolCallContext, UpdateChannelInput,
    VaultSaveInput, all_provider_harnesses, build_handoff_packet, compute_totp, generate_pkce_s256,
    verify_pkce_s256,
};
use openbot_storage::{
    AgentSqlMode, McpTransport, MemoryInclusion, SendDirectMessageInput, direct_thread_id,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

fn make_agent(host: &OpenBotHostRuntime, name: &str) -> openbot_contracts::agents::AgentSummary {
    host.agents()
        .create_agent_full(CreateAgentRequest {
            name: name.to_string(),
            title: format!("{name} specialist"),
            description: format!("Helper agent {name}"),
            provider: Some(AgentProviderId::Codex),
            model: Some("gpt-5.4".to_string()),
            access: Some(AgentAccess::Workspace),
            computer_use: Some(false),
            allow_automation: Some(false),
            ..CreateAgentRequest::default()
        })
        .expect("create_agent_full")
}

#[test]
fn ob_f06_providers_failover_lifecycle_dup_import_export_and_privilege_guard() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");

    // 1. All 18 built-in provider harnesses + zero starter bots on empty DB
    assert_eq!(host.agents().list_agents().len(), 0);
    let harnesses = all_provider_harnesses();
    assert_eq!(harnesses.len(), 18);
    assert!(harnesses.iter().any(|h| h.id == AgentProviderId::Codex));
    assert!(harnesses.iter().any(|h| h.id == AgentProviderId::Muse && !h.supports_ephemeral_toolless));
    assert!(harnesses.iter().any(|h| h.id == AgentProviderId::Acp && !h.descriptor.concurrent_turns));

    // 2. Custom endpoints (reserved ID refused, secrets never exposed in summary)
    let reserved = host.providers().save_custom_provider(CustomProviderEndpoint {
        id: "codex".to_string(),
        name: "Reserved".to_string(),
        base_url: "https://api.example.com/v1".to_string(),
        api_key: Some("sk-reserved-1234567890".to_string()),
        models: vec![serde_json::from_value::<CustomProviderModel>(json!({
            "id": "m1",
            "name": "M1"
        }))
        .unwrap()],
        headers: Vec::new(),
        harness: Some(CustomProviderHarness::OpenCode),
    });
    assert!(reserved.is_err());

    let saved_custom = host
        .providers()
        .save_custom_provider(CustomProviderEndpoint {
            id: "my-local-gw".to_string(),
            name: "Local Gateway".to_string(),
            base_url: "https://gateway.example.com/v1".to_string(),
            api_key: Some("sk-super-secret-custom-999999".to_string()),
            models: vec![serde_json::from_value::<CustomProviderModel>(json!({
                "id": "custom-coder",
                "name": "Custom Coder"
            }))
            .unwrap()],
            headers: Vec::new(),
            harness: Some(CustomProviderHarness::OpenCode),
        })
        .expect("save custom provider");
    assert_eq!(saved_custom.providers.len(), 1);
    let summaries_json = serde_json::to_string(&host.providers().list_custom_providers()).unwrap();
    assert!(!summaries_json.contains("sk-super-secret-custom-999999"));

    // 3. <= 8 backup models cap + rate-limit failover + usageLimitReturn + <= 32k handoff packet
    let agent = make_agent(&host, "Atlas");
    let too_many_backups = AgentBackupModels {
        automatic: true,
        order: (0..9)
            .map(|i| AgentModelChoice {
                provider: AgentProviderId::Claude,
                model: format!("claude-sonnet-{i}"),
                provider_account_id: None,
            })
            .collect(),
    };
    let cap_err = host.agents().update_agent(
        &UpdateAgentInput {
            agent_id: agent.id.clone(),
            backup_models: Some(Some(too_many_backups)),
            ..UpdateAgentInput::default()
        },
        None,
    );
    assert!(cap_err.is_err());

    let agent_with_backup = host
        .agents()
        .update_agent(
            &UpdateAgentInput {
                agent_id: agent.id.clone(),
                backup_models: Some(Some(AgentBackupModels {
                    automatic: true,
                    order: vec![AgentModelChoice {
                        provider: AgentProviderId::Claude,
                        model: "claude-sonnet-4-6".to_string(),
                        provider_account_id: None,
                    }],
                })),
                ..UpdateAgentInput::default()
            },
            None,
        )
        .expect("save backup model");

    let big_msgs: Vec<ConversationMessage> = (0..40)
        .map(|i| {
            serde_json::from_value::<ConversationMessage>(json!({
                "id": format!("m-{i}"),
                "author": "user",
                "text": format!("Turn {i} details {}", "x".repeat(1200)),
                "createdAt": "2026-11-26T00:00:00.000Z",
                "status": "completed",
                "source": "user"
            }))
            .unwrap()
        })
        .collect();
    let handoff = build_handoff_packet(&big_msgs, Some("step-1"), "finish deployment");
    assert!(handoff.starts_with(HANDOFF_START));
    assert!(handoff.encode_utf16().count() <= HANDOFF_MAX_CHARS);

    let failover = host
        .providers()
        .execute_turn_with_failover(&StartTurnRequest {
            agent: agent_with_backup.clone(),
            thread_id: "thread-failover-1".to_string(),
            prompt: "summarize release".to_string(),
            handoff_prompt: Some(handoff),
            simulate_primary_rate_limit: true,
        })
        .expect("backup failover");
    assert_eq!(failover.active_provider, AgentProviderId::Claude);
    assert_eq!(failover.active_model, "claude-sonnet-4-6");
    let ulr = failover.usage_limit_return.expect("usage_limit_return");
    assert_eq!(ulr.provider, AgentProviderId::Codex);
}

#[test]
fn ob_f06_muse_refusal_duplication_import_export_privilege_guard_and_cap() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");

    // 1. Muse refusal on generate_profile vs valid Codex generate_profile + welcome delivery
    let muse_err = host.agents().generate_profile(&GenerateProfileRequest {
        prompt: "Build a release helper".to_string(),
        provider: Some(AgentProviderId::Muse),
        model: None,
        agent_id: None,
        draft: None,
        save: false,
        operation_id: None,
        language: "en".to_string(),
        section_id: None,
        first_request_follows: false,
    });
    assert!(muse_err.is_err());

    let generated = host
        .agents()
        .generate_profile(&GenerateProfileRequest {
            prompt: "Build a database migration assistant".to_string(),
            provider: Some(AgentProviderId::Codex),
            model: Some("gpt-5.4".to_string()),
            agent_id: None,
            draft: None,
            save: true,
            operation_id: Some(uuid::Uuid::new_v4().to_string()),
            language: "en".to_string(),
            section_id: None,
            first_request_follows: false,
        })
        .expect("generate_profile save");
    let src = generated.agent.expect("saved generated agent");
    let welcome_delivery_id = generated
        .welcome_delivery_id
        .clone()
        .expect("welcome_delivery_id");
    let welcome_queue = host.scheduler().queue_list(&src.id).expect("queue_list");
    assert!(welcome_queue.welcome);
    assert_eq!(welcome_queue.items.len(), 1);
    assert!(welcome_queue.items[0].welcome);
    assert_eq!(welcome_queue.items[0].delivery_id, welcome_delivery_id);
    assert!(welcome_queue.items[0].text.contains("openbot.ask_user"));

    // 2. Privilege-widening refusal via direct AgentLifecycleService AND OpenBotToolRouter
    let restricted = make_agent(&host, "RestrictedBot");
    let widen_direct = host.agents().update_agent(
        &UpdateAgentInput {
            agent_id: restricted.id.clone(),
            access: Some(AgentAccess::Full),
            ..UpdateAgentInput::default()
        },
        Some(&restricted.id),
    );
    assert!(matches!(widen_direct, Err(OpenBotRuntimeError::Forbidden(_))));

    let router_widen = host.tool_router().call_tool_value(
        &ToolCallContext::for_agent(&restricted.id),
        "openbot.update_profile",
        json!({ "access": "full", "computerUse": true, "allowAutomation": true }),
    );
    assert!(matches!(router_widen, Err(OpenBotRuntimeError::Forbidden(_))));

    // 3. .openbot-stage atomic duplication + concurrent mutation rollback with zero leftovers
    fs::write(
        std::path::Path::new(&src.workspace_path).join("notes.txt"),
        "important workspace note",
    )
    .expect("write note");
    let conflict = host
        .agents()
        .duplicate_agent(&src.id, &uuid::Uuid::new_v4().to_string(), true);
    assert!(matches!(conflict, Err(OpenBotRuntimeError::Conflict(_))));
    let agents_dir = dir.path().join("Agents");
    let has_stage = fs::read_dir(&agents_dir)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".openbot-stage"));
    assert!(!has_stage, "concurrent mutation rollback must leave zero .openbot-stage dirs");

    let dup_op_id = uuid::Uuid::new_v4().to_string();
    let dup = host
        .agents()
        .duplicate_agent(&src.id, &dup_op_id, false)
        .expect("duplicate_agent");
    // Verify idempotency on replay of same UUID v4 op_id
    let dup_replay = host
        .agents()
        .duplicate_agent(&src.id, &dup_op_id, false)
        .expect("duplicate_agent replay");
    assert_eq!(dup.id, dup_replay.id);
    assert_ne!(dup.id, src.id);
    assert_eq!(
        fs::read_to_string(std::path::Path::new(&dup.workspace_path).join("notes.txt")).unwrap(),
        "important workspace note"
    );

    // 4. .openbot archive export/import + Grok .zip + Hermes import
    let bundle_path = dir.path().join("export.openbot");
    let exported = host
        .agents()
        .export_archive(&[src.id.clone()], &bundle_path)
        .expect("export_archive");
    assert_eq!(exported.exported_count, 1);

    let dir2 = TempDir::new().expect("tempdir2");
    let host2 = OpenBotHostRuntime::open(dir2.path()).expect("host2");
    let imported = host2
        .agents()
        .import_archive(&bundle_path, None)
        .expect("import_archive");
    assert_eq!(imported.agents.len(), 1);

    // Grok .zip import
    let grok_zip = dir2.path().join("grok-export.zip");
    {
        let f = File::create(&grok_zip).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        zw.start_file("prod-grok-backend.json", SimpleFileOptions::default())
            .unwrap();
        zw.write_all(
            br#"{"projects":[{"id":"p1","name":"Grok Analyst","instructions":"Analyze quarterly KPIs"}]}"#,
        )
        .unwrap();
        zw.finish().unwrap();
    }
    let grok_res = host2.agents().import_grok_zip(&grok_zip).expect("import_grok_zip");
    assert!(!grok_res.agents.is_empty());

    // Hermes directory import
    let hermes_dir = dir2.path().join("hermes-home");
    fs::create_dir_all(hermes_dir.join("memories")).unwrap();
    fs::write(hermes_dir.join("SOUL.md"), "# Hermes Bot\nHelpful ops agent.").unwrap();
    fs::write(hermes_dir.join("memories/MEMORY.md"), "Prefers concise summaries").unwrap();
    let hermes_res = host2
        .agents()
        .import_hermes(&hermes_dir, None)
        .expect("import_hermes");
    assert!(!hermes_res.agents.is_empty());

    // 5. <= 100 agent cap
    let cap_dir = TempDir::new().expect("cap_dir");
    let cap_host = OpenBotHostRuntime::open(cap_dir.path()).expect("cap_host");
    for i in 0..100 {
        let _ = make_agent(&cap_host, &format!("Bot-{i:03}"));
    }
    assert_eq!(cap_host.agents().list_agents().len(), 100);
    let over_cap = cap_host.agents().create_agent_full(CreateAgentRequest {
        name: "Bot-101".to_string(),
        provider: Some(AgentProviderId::Codex),
        ..CreateAgentRequest::default()
    });
    assert!(matches!(over_cap, Err(OpenBotRuntimeError::LimitExceeded(_))));
}

#[test]
fn ob_f07_compaction_attention_scheduler_mailbox_and_dms() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a1 = make_agent(&host, "Alpha");
    let a2 = make_agent(&host, "Beta");
    let acp_agent = host
        .agents()
        .create_agent_full(CreateAgentRequest {
            name: "AcpSingle".to_string(),
            provider: Some(AgentProviderId::Acp),
            model: Some("acp-default".to_string()),
            ..CreateAgentRequest::default()
        })
        .expect("acp agent");

    // 1. Multi-lane limit: concurrent provider > 1, ACP strictly 1
    assert!(host.scheduler().effective_task_limit(&a1) >= 1);
    assert_eq!(host.scheduler().effective_task_limit(&acp_agent), 1);

    // 2. Send chat messages + 80% ContextCompactor
    let sent = host
        .scheduler()
        .send_message(SendChatMessageInput {
            agent_id: a1.id.clone(),
            text: "Prepare deployment checklist for v2.0".to_string(),
            ..SendChatMessageInput::default()
        })
        .expect("send_message");
    let budget = host.compactor().observe_usage(&sent.thread_id, 105_000, 1_000, 128_000);
    assert!(budget.pending && budget.ratio >= 0.80);
    let compact = host
        .scheduler()
        .compact_context(&a1.id, None, Some("105000/128000"), CompactionReason::Threshold)
        .expect("compact_context");
    assert_eq!(compact.status.as_str(), "completed");

    // 3. AttentionRegistry: 10m TTL approval + ask_ui Secret redaction
    let approval = host
        .attention()
        .request_approval_at(
            ApprovalRequestInput {
                agent_id: a1.id.clone(),
                thread_id: sent.thread_id.clone(),
                turn_id: Some(sent.turn_id.clone()),
                kind: ApprovalKind::Command,
                action: "deploy:prod".to_string(),
                title: "Run production deploy".to_string(),
                detail: None,
                command: Some("deploy --prod".to_string()),
                cwd: None,
                reason: Some("Release".to_string()),
                owner_only: true,
                ttl_ms: None,
            },
            1_000_000,
        )
        .expect("request_approval");
    assert_eq!(approval.expires_at_ms - approval.created_at_ms, 600_000);
    let resolved_ap = host
        .attention()
        .respond_approval_at(&approval.request_id, ApprovalDecision::Accept, None, true, 1_010_000)
        .expect("respond_approval");
    assert_eq!(resolved_ap.decision, Some(ApprovalDecision::Accept));

    let secret_form: UiBlockSpec = serde_json::from_value(json!({
        "type": "form",
        "title": "API Credentials",
        "fields": [
            { "id": "api_key", "label": "API Key", "kind": "text", "secret": true, "required": true }
        ],
        "submit": "Save"
    }))
    .expect("form spec");
    let ui_rec = host
        .attention()
        .ask_ui(AskUiInput {
            agent_id: a1.id.clone(),
            thread_id: sent.thread_id.clone(),
            turn_id: Some(sent.turn_id.clone()),
            block_id: None,
            spec: secret_form,
            ttl_ms: None,
        })
        .expect("ask_ui");
    let raw_secret = "sk-live-secret-never-persist-987654";
    let ui_resp: UiBlockResponse = serde_json::from_value(json!({
        "actionId": "submit",
        "values": { "api_key": raw_secret }
    }))
    .expect("ui response");
    let ui_done = host
        .attention()
        .respond_ui_block(&ui_rec.request_id, ui_resp, None, true)
        .expect("respond_ui_block");
    let handle = ui_done.secret_handles.get("api_key").expect("secret handle");
    assert_eq!(handle.redaction_marker, "[REDACTED]");
    let attention_disk =
        fs::read_to_string(dir.path().join("openbot-attention-v1.json")).unwrap_or_default();
    assert!(!attention_disk.contains(raw_secret));
    assert_eq!(
        host.attention().take_secret_by_handle(&handle.handle_id).as_deref(),
        Some(raw_secret)
    );
    assert!(host.attention().take_secret_by_handle(&handle.handle_id).is_none());

    // 4. Queue add, 10m edit hold, pause/resume, Tab steer routine refusal
    let _ = host.scheduler().queue_pause(&a1.id).expect("queue_pause");
    let q1 = host
        .scheduler()
        .queue_add(QueueAddInput {
            agent_id: a1.id.clone(),
            text: "First queued follow-up".to_string(),
            ..QueueAddInput::default()
        })
        .expect("queue_add 1");
    let q2 = host
        .scheduler()
        .queue_add(QueueAddInput {
            agent_id: a1.id.clone(),
            text: "Second queued follow-up".to_string(),
            ..QueueAddInput::default()
        })
        .expect("queue_add 2");

    let edit_begin = host
        .scheduler()
        .queue_edit(
            &a1.id,
            QueueEditRequest::Begin {
                delivery_id: q2.delivery_id.clone(),
                edit_id: "edit-1".to_string(),
            },
            None,
        )
        .expect("queue_edit begin");
    assert_eq!(edit_begin.hold_ms, 600_000);
    let _ = host
        .scheduler()
        .queue_edit(
            &a1.id,
            QueueEditRequest::Save {
                delivery_id: q2.delivery_id.clone(),
                edit_id: "edit-1".to_string(),
                text: "Second queued follow-up (edited)".to_string(),
                keep_attachment_ids: Vec::new(),
                attachment_draft_ids: Vec::new(),
            },
            None,
        )
        .expect("queue_edit save");

    let resumed = host.scheduler().queue_resume(&a1.id).expect("queue_resume");
    let active_turn = resumed.active_turn_id.clone().expect("active_turn_id");
    let steer_routine = host
        .scheduler()
        .queue_steer(QueueSteerInput {
            agent_id: a1.id.clone(),
            delivery_id: q2.delivery_id.clone(),
            expected_turn_id: active_turn.clone(),
            routine_turn: true,
        })
        .expect("queue_steer routine");
    assert!(steer_routine.steer_refused && !steer_routine.accepted);
    assert_ne!(q1.delivery_id, q2.delivery_id);

    // 5. Branch thread (branchSourceBusy while active turn running, succeeds after draining) + zero-DB ask_side_question
    let branch_busy = host
        .scheduler()
        .branch_thread(openbot_runtime::BranchThreadInput {
            agent_id: a1.id.clone(),
            message_id: sent.message_id.clone(),
            request_id: "branch-req-busy".to_string(),
            ..Default::default()
        });
    assert!(matches!(branch_busy, Err(OpenBotRuntimeError::Conflict(_))));
    let _ = host.scheduler().drain_agent_fifo(&a1.id).expect("drain_agent_fifo");

    let branch = host
        .scheduler()
        .branch_thread(openbot_runtime::BranchThreadInput {
            agent_id: a1.id.clone(),
            message_id: sent.message_id.clone(),
            request_id: "branch-req-1".to_string(),
            ..Default::default()
        })
        .expect("branch_thread");
    assert!(branch.task_id.starts_with("openbot-thread-"));

    let events_before: i64 = host
        .database()
        .with_conn(|c| Ok(c.query_row("SELECT COUNT(*) FROM orchestration_events", [], |r| r.get(0))?))
        .unwrap();
    let side_ans = host
        .scheduler()
        .ask_side_question(SideQuestionInput {
            agent_id: a1.id.clone(),
            question: "What did we decide?".to_string(),
            ..Default::default()
        })
        .expect("ask_side_question");
    assert!(!side_ans.answer.is_empty());
    let events_after: i64 = host
        .database()
        .with_conn(|c| Ok(c.query_row("SELECT COUNT(*) FROM orchestration_events", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(events_before, events_after, "ask_side_question must write 0 DB events");

    // 6. RGI reactions, pinned-by-person & 50-pin cap
    let reacted = host
        .scheduler()
        .react_message(ReactMessageInput {
            agent_id: a1.id.clone(),
            task_id: None,
            channel_id: None,
            message_id: sent.message_id.clone(),
            emoji: "👍".to_string(),
            actor: ConversationReactionActor::User,
            remove: false,
        })
        .expect("react_message");
    assert_eq!(reacted.emoji.as_deref(), Some("👍"));

    let _ = host
        .scheduler()
        .pin_message(PinMessageInput {
            agent_id: a1.id.clone(),
            task_id: None,
            channel_id: None,
            message_id: sent.message_id.clone(),
            actor: PinActor::Person { sender: None },
            unpin: false,
        })
        .expect("pin by person");
    let unpin_by_agent = host.scheduler().pin_message(PinMessageInput {
        agent_id: a1.id.clone(),
        task_id: None,
        channel_id: None,
        message_id: sent.message_id.clone(),
        actor: PinActor::Agent,
        unpin: true,
    });
    assert!(matches!(unpin_by_agent, Err(OpenBotRuntimeError::Forbidden(_))));

    // 7. Mailbox 1..=32 barrier + interrupt_agent + dm-<sha256> DMs
    let too_many_recips: Vec<String> = (0..33).map(|i| format!("bot-{i}")).collect();
    assert!(
        host.scheduler()
            .mailbox_send(MailboxSendInput {
                sender_agent_id: a1.id.clone(),
                recipient_agent_ids: too_many_recips,
                text: "fanout".to_string(),
                ..Default::default()
            })
            .is_err()
    );
    let mb = host
        .scheduler()
        .mailbox_send(MailboxSendInput {
            sender_agent_id: a1.id.clone(),
            recipient_agent_ids: vec![a2.id.clone()],
            text: "Status: done\nResult: all checks passed\nEvidence: ci#42".to_string(),
            expects_reply: Some(false),
            ..Default::default()
        })
        .expect("mailbox_send");
    assert!(mb.answer_optional && !mb.expects_reply);
    assert_eq!(mb.parsed_reply.as_ref().map(|r| r.status.as_str()), Some("done"));

    let interrupted = host
        .scheduler()
        .interrupt_agent(InterruptAgentInput {
            caller_agent_id: a1.id.clone(),
            target_agent_id: a2.id.clone(),
            reason: Some("Cancel pending".to_string()),
            ..Default::default()
        })
        .expect("interrupt_agent");
    assert!(interrupted.cancelled_queued_count >= 1);

    let dm = host
        .scheduler()
        .dm_send(SendDirectMessageInput {
            sender_member_id: "user-alice".to_string(),
            recipient_member_id: "user-bob".to_string(),
            client_message_id: "dm-client-1".to_string(),
            text: "Hey Bob!".to_string(),
            created_at: None,
        })
        .expect("dm_send");
    assert_eq!(dm.thread_id, direct_thread_id("user-alice", "user-bob"));
    assert!(dm.thread_id.starts_with("dm-") && dm.thread_id.len() == 35);
    assert_eq!(host.scheduler().dm_list("user-alice").unwrap().len(), 1);
}

#[test]
fn ob_f07_fifty_pin_limit_guard() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a1 = make_agent(&host, "Pinner");
    let mut msg_ids = Vec::with_capacity(51);
    for i in 0..51 {
        let sent = host
            .scheduler()
            .send_message(SendChatMessageInput {
                agent_id: a1.id.clone(),
                text: format!("Pinnable item #{i}"),
                ..Default::default()
            })
            .expect("send_message");
        msg_ids.push(sent.message_id);
    }
    for mid in &msg_ids[..50] {
        host.scheduler()
            .pin_message(PinMessageInput {
                agent_id: a1.id.clone(),
                task_id: None,
                channel_id: None,
                message_id: mid.clone(),
                actor: PinActor::Person { sender: None },
                unpin: false,
            })
            .expect("pin <= 50");
    }
    let pin51 = host.scheduler().pin_message(PinMessageInput {
        agent_id: a1.id.clone(),
        task_id: None,
        channel_id: None,
        message_id: msg_ids[50].clone(),
        actor: PinActor::Person { sender: None },
        unpin: false,
    });
    assert!(matches!(pin51, Err(OpenBotRuntimeError::LimitExceeded(_))));
}

#[test]
fn ob_f08_channel_crud_contexts_mentions_router_and_followups() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let lead = make_agent(&host, "LeadBot");
    let renee = make_agent(&host, "Renée");
    let renee_ops = make_agent(&host, "Renée Ops");
    let outsider = make_agent(&host, "Outsider");

    // 1. 1-member fast path (0 router invocations, eager projection_channel_contexts)
    let solo = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Solo Room".to_string(),
            title: "Solo".to_string(),
            instructions: "Fast path".to_string(),
            members: vec![ChannelMember {
                agent_id: lead.id.clone(),
            }],
            lead_agent_id: Some(lead.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create solo channel");
    let solo_ctx_count: i64 = host
        .database()
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM projection_channel_contexts WHERE channel_id = ?1",
                rusqlite::params![solo.id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(solo_ctx_count, 1);

    let solo_post = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: solo.id.clone(),
            text: "Quick status check".to_string(),
            ..Default::default()
        })
        .expect("solo post");
    assert_eq!(solo_post.router_invocations, 0);
    assert_eq!(solo_post.tasks.len(), 1);

    // 2. Multi-member channel: eager contexts, NFD rank_mentions 0..=6, @[Name](agent:id), @everyone, longest-match
    let team = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Release Squad".to_string(),
            title: "Multi-bot".to_string(),
            instructions: "Coordinate release".to_string(),
            members: vec![
                ChannelMember { agent_id: lead.id.clone() },
                ChannelMember { agent_id: renee.id.clone() },
                ChannelMember { agent_id: renee_ops.id.clone() },
            ],
            lead_agent_id: Some(lead.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create team channel");

    let team_ctx_count: i64 = host
        .database()
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM projection_channel_contexts WHERE channel_id = ?1",
                rusqlite::params![team.id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(team_ctx_count, 3);

    let ranked = host.channels().rank_mentions(&team.id, "renee").expect("rank_mentions");
    assert!(ranked.iter().all(|r| r.agent_id != outsider.id));
    assert_eq!(ranked.first().map(|r| r.agent_id.as_str()), Some(renee.id.as_str()));
    assert_eq!(ranked.first().map(|r| r.rank), Some(0));

    // Longest-match @Renée Ops wins over @Renée
    let longest = host
        .channels()
        .resolve_recipients(&team, "@Renée Ops check metrics", None)
        .expect("longest match");
    assert_eq!(longest, vec![renee_ops.id.clone()]);

    // @everyone fans out to all 3 members
    let all_recips = host
        .channels()
        .resolve_recipients(&team, "@everyone give update", None)
        .expect("@everyone");
    assert_eq!(all_recips.len(), 3);

    // Structured @[Renée](agent:<id>)
    let structured = host
        .channels()
        .resolve_recipients(&team, &format!("@[Renée](agent:{}) check logs", renee.id), None)
        .expect("structured mention");
    assert_eq!(structured, vec![renee.id.clone()]);

    // 3. Lead JSON router: m1..mN, stale routingState repump, and idle mid-text @ override
    let idle_override = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: team.id.clone(),
            text: "Please ask @Renée to verify the changelog".to_string(),
            router_mock: Some(r#"{"idle":true}"#.to_string()),
            simulate_stale_routing_state: true,
            ..Default::default()
        })
        .expect("lead router post");
    assert!(idle_override.router_invocations >= 1);
    assert!(idle_override.repumped);
    assert!(
        idle_override
            .tasks
            .iter()
            .any(|t| t.task.owner_agent_id.as_deref() == Some(renee.id.as_str()))
    );

    // Complete that task and post a follow-up to verify superseded: true on prior message
    let first_task = idle_override.tasks[0].task.clone();
    let res1 = host
        .channels()
        .result(ChannelResultInput {
            channel_id: team.id.clone(),
            task_id: first_task.id.clone(),
            agent_id: first_task.owner_agent_id.clone().unwrap(),
            text: format!("{}: Initial changelog draft.", renee.name),
            turn_id: None,
            operation_id: None,
        })
        .expect("result 1");
    assert!(!res1.text.starts_with("Renée:"));

    let _followup = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: team.id.clone(),
            text: "actually add the security section too".to_string(),
            reply_to_message_id: Some(res1.message_id.clone()),
            ..Default::default()
        })
        .expect("followup");
    let detail = host.channels().get_detail(&team.id).expect("get_detail");
    let prior = detail
        .messages
        .iter()
        .find(|m| m.id == res1.message_id)
        .expect("prior msg");
    assert!(prior.superseded);

    // Update + archive/restore read-only guard
    let _ = host
        .channels()
        .update(UpdateChannelInput {
            channel_id: solo.id.clone(),
            title: Some("Solo Updated".to_string()),
            ..Default::default()
        })
        .expect("update channel");
    let _ = host
        .channels()
        .result(ChannelResultInput {
            channel_id: solo.id.clone(),
            task_id: solo_post.tasks[0].task.id.clone(),
            agent_id: lead.id.clone(),
            text: "Done".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("finish solo task");
    let archived = host.channels().archive(&solo.id, None).expect("archive");
    assert!(archived.archived);
    assert!(
        host.channels()
            .post(PostChannelMessageInput {
                channel_id: solo.id.clone(),
                text: "blocked".to_string(),
                ..Default::default()
            })
            .is_err()
    );
    let restored = host.channels().restore(&solo.id, None).expect("restore");
    assert!(!restored.archived);
}

#[test]
fn ob_f09_channel_delegation_limits_quarantine_dag_mutex_and_history_packets() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a = make_agent(&host, "Alice");
    let b = make_agent(&host, "Bob");
    let c = make_agent(&host, "Carol");

    assert_eq!(CHANNEL_PARALLEL_LIMIT, 2);
    assert_eq!(CHANNEL_ASSIGNMENT_LIMIT, 8);
    assert_eq!(CHANNEL_REPLY_LIMIT, 2);

    let ch = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Ops War Room".to_string(),
            title: "Delegation tests".to_string(),
            instructions: "Work together".to_string(),
            members: vec![
                ChannelMember { agent_id: a.id.clone() },
                ChannelMember { agent_id: b.id.clone() },
                ChannelMember { agent_id: c.id.clone() },
            ],
            lead_agent_id: Some(a.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create channel");

    // 1. Parallel limit = 2 + sibling hiding + full/delta history packet + 8k/12k summary
    let fanout = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: ch.id.clone(),
            text: "@everyone produce independent audit notes".to_string(),
            hold_running: true,
            ..Default::default()
        })
        .expect("@everyone post");
    assert_eq!(fanout.tasks.len(), 3);
    let running_count = fanout
        .tasks
        .iter()
        .filter(|t| t.task.state == ChannelTaskState::Running)
        .count();
    assert_eq!(running_count, CHANNEL_PARALLEL_LIMIT);

    let t_a = fanout
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(a.id.as_str()))
        .unwrap()
        .task
        .clone();
    let t_b = fanout
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(b.id.as_str()))
        .unwrap()
        .task
        .clone();
    let t_c = fanout
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(c.id.as_str()))
        .unwrap()
        .task
        .clone();

    // Finish A's answer, then verify B's first-turn packet hides sibling A's answer!
    let res_a = host
        .channels()
        .result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: t_a.id.clone(),
            agent_id: a.id.clone(),
            text: "Alice: SECRET_SIBLING_ANSWER_FROM_ALICE".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("result A");
    assert_eq!(res_a.text, "SECRET_SIBLING_ANSWER_FROM_ALICE");

    let pkt_b_first = host
        .channels()
        .history_packet(ChannelHistoryPacketInput {
            channel_id: ch.id.clone(),
            task_id: Some(t_b.id.clone()),
            agent_id: b.id.clone(),
            second_turn: false,
            simulate_large_transcript: false,
            resumed: false,
            requested_budget: None,
            mode_override: None,
        })
        .expect("history packet B first");
    assert_eq!(pkt_b_first.kind, "full");
    assert!(pkt_b_first.hidden_sibling_count >= 1);
    assert!(!pkt_b_first.text.contains("SECRET_SIBLING_ANSWER_FROM_ALICE"));

    let pkt_b_delta = host
        .channels()
        .history_packet(ChannelHistoryPacketInput {
            channel_id: ch.id.clone(),
            task_id: Some(t_b.id.clone()),
            agent_id: b.id.clone(),
            second_turn: true,
            simulate_large_transcript: true,
            resumed: false,
            requested_budget: None,
            mode_override: None,
        })
        .expect("history packet B delta");
    assert_eq!(pkt_b_delta.kind, "delta");
    assert!(pkt_b_delta.summary_chars <= 12_000);

    // 2. channel_reply quarantine + failed-reply deadlock prevention
    let reply_fail = host
        .channels()
        .reply(ChannelReplyInput {
            channel_id: ch.id.clone(),
            task_id: t_b.id.clone(),
            agent_id: b.id.clone(),
            message_id: res_a.message_id.clone(),
            text: "Alice, can you confirm the checksum?".to_string(),
            turn_id: None,
            simulate_peer_failure: true,
            operation_id: None,
        })
        .expect("reply with peer failure");
    assert!(reply_fail.is_reply_task);
    assert!(reply_fail.caller_resumed, "failed reply must release caller to prevent deadlock");

    // Finish B & C so we can test assign, transfer, DAG cycle, assignment limit = 8, continue/reassign/cancel, and resource mutexes
    let _ = host.channels().result(ChannelResultInput {
        channel_id: ch.id.clone(),
        task_id: t_b.id.clone(),
        agent_id: b.id.clone(),
        text: "Bob done".to_string(),
        turn_id: None,
        operation_id: None,
    });
    let _ = host.channels().result(ChannelResultInput {
        channel_id: ch.id.clone(),
        task_id: t_c.id.clone(),
        agent_id: c.id.clone(),
        text: "Carol done".to_string(),
        turn_id: None,
        operation_id: None,
    });

    // 3. Resource mutex ("browser" & "workspace:<path>")
    let mut rmap = HashMap::new();
    rmap.insert(a.id.clone(), vec!["browser".to_string(), "workspace:/repo/sub".to_string()]);
    rmap.insert(b.id.clone(), vec!["browser".to_string(), "workspace:/repo".to_string()]);
    let mutex_post = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: ch.id.clone(),
            text: format!("@[Alice](agent:{}) @[Bob](agent:{}) check checkout page", a.id, b.id),
            hold_running: true,
            resources_map: rmap,
            ..Default::default()
        })
        .expect("mutex post");
    assert!(
        mutex_post.tasks.iter().any(|t| t.task.state == ChannelTaskState::Queued),
        "conflicting browser/workspace mutex must queue second task"
    );
    let m_a = mutex_post
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(a.id.as_str()))
        .unwrap()
        .task
        .clone();
    let m_b = mutex_post
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(b.id.as_str()))
        .unwrap()
        .task
        .clone();

    let _ = host
        .channels()
        .result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: m_a.id.clone(),
            agent_id: a.id.clone(),
            text: "Alice finished browser check".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("finish m_a");

    // 4. DAG cycle guard + assign + transfer + CHANNEL_ASSIGNMENT_LIMIT=8 + continue/reassign/cancel
    let chain_post = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: ch.id.clone(),
            text: format!("@[Alice](agent:{}) coordinate billing audit", a.id),
            hold_running: true,
            ..Default::default()
        })
        .expect("chain post");
    let chain_root = chain_post.tasks[0].task.clone();

    let self_cycle = host.channels().assign(ChannelAssignInput {
        channel_id: ch.id.clone(),
        caller_task_id: chain_root.id.clone(),
        agent_id: a.id.clone(),
        recipient_agent_id: c.id.clone(),
        task: "Check cycle".to_string(),
        expected_result: Some("ok".to_string()),
        source_message_ids: vec![chain_post.message_id.clone()],
        resources: Vec::new(),
        dependencies: vec![chain_root.id.clone()],
        operation_id: None,
    });
    assert!(self_cycle.is_err(), "dependsOn cycle through caller task must be rejected");

    let assigned = host
        .channels()
        .assign(ChannelAssignInput {
            channel_id: ch.id.clone(),
            caller_task_id: chain_root.id.clone(),
            agent_id: a.id.clone(),
            recipient_agent_id: c.id.clone(),
            task: "Audit billing module".to_string(),
            expected_result: Some("Audit summary".to_string()),
            source_message_ids: vec![chain_post.message_id.clone()],
            resources: Vec::new(),
            dependencies: Vec::new(),
            operation_id: None,
        })
        .expect("assign");
    assert_eq!(assigned.parent_state, "waiting");

    // Transfer chain until CHANNEL_ASSIGNMENT_LIMIT = 8 trips LimitExceeded & pauses task on 9th handoff
    let mut cur_task = assigned.task.id.clone();
    let mut cur_owner = c.id.clone();
    let mut paused_task_id = None;
    for step in 0..9 {
        let next_owner = if cur_owner == c.id { b.id.clone() } else { c.id.clone() };
        match host.channels().transfer(ChannelTransferInput {
            channel_id: ch.id.clone(),
            task_id: cur_task.clone(),
            agent_id: cur_owner.clone(),
            recipient_agent_id: next_owner.clone(),
            task: format!("Handoff step {step}"),
            expected_result: Some("done".to_string()),
            source_message_ids: vec![chain_post.message_id.clone()],
            resources: Vec::new(),
            dependencies: Vec::new(),
            operation_id: None,
        }) {
            Ok(tr) => {
                cur_task = tr.task.id.clone();
                cur_owner = tr.task.owner_agent_id.clone().unwrap();
                if tr.task.state == ChannelTaskState::Paused {
                    paused_task_id = Some(tr.task.id);
                    break;
                }
            }
            Err(OpenBotRuntimeError::LimitExceeded(_)) => {
                let detail = host.channels().get_detail(&ch.id).expect("detail after limit");
                let paused = detail
                    .tasks
                    .iter()
                    .find(|t| t.task.id == cur_task)
                    .expect("paused task present");
                assert_eq!(paused.task.state, ChannelTaskState::Paused);
                paused_task_id = Some(cur_task.clone());
                break;
            }
            Err(other) => panic!("unexpected transfer error: {other}"),
        }
    }
    let paused_id = paused_task_id.expect("9th handoff must pause task at CHANNEL_ASSIGNMENT_LIMIT=8");

    // Reassign paused task to a different member, then continue & cancel
    let other_member = if cur_owner == b.id { c.id.clone() } else { b.id.clone() };
    let reassigned = host
        .channels()
        .reassign(ChannelReassignInput {
            channel_id: ch.id.clone(),
            task_id: paused_id.clone(),
            recipient_agent_id: other_member.clone(),
            operation_id: None,
        })
        .expect("reassign");
    assert_eq!(reassigned.owner_agent_id.as_deref(), Some(other_member.as_str()));
    assert_eq!(reassigned.assignment_count, 0);

    let stopped = host.channels().stop_task(&ch.id, &reassigned.id, None).expect("stop_task");
    assert_eq!(stopped.state, ChannelTaskState::Paused);
    let continued = host
        .channels()
        .continue_task(ChannelContinueInput {
            channel_id: ch.id.clone(),
            task_id: stopped.id.clone(),
            recipient_agent_id: None,
            operation_id: None,
        })
        .expect("continue_task");
    assert_ne!(continued.state, ChannelTaskState::Paused);

    let _ = host.channels().stop_task(&ch.id, &m_b.id, None).expect("stop m_b");
    let cancelled = host
        .channels()
        .cancel(ChannelCancelInput {
            channel_id: ch.id.clone(),
            task_id: m_b.id.clone(),
            caller_agent_id: None,
            caller_task_id: None,
            origin_routine_id: None,
            operation_id: None,
        })
        .expect("cancel stopped task");
    assert_eq!(cancelled.task.state, ChannelTaskState::Cancelled);
}

#[test]
fn ob_f10_catalog_router_memory_isolation_sql_skills_mcp_vault_and_browser() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a1 = make_agent(&host, "ToolMaster");
    let a2 = make_agent(&host, "OtherBot");

    // 1. 160-tool ToolCatalog + search/describe/call guards
    let listed = host.tool_router().list_tools(None, true);
    assert_eq!(listed.direct_openbot_count, DIRECT_OPENBOT_COUNT);
    assert_eq!(listed.direct_browser_count, DIRECT_BROWSER_COUNT);
    assert_eq!(listed.channel_tool_count, CHANNEL_TOOLS_COUNT);
    assert_eq!(listed.catalog_total, FULL_CATALOG_COUNT);

    let searched = host.tool_router().search_tools("memory", 5).expect("search_tools");
    assert!(!searched.tools.is_empty());
    let described = host
        .tool_router()
        .describe_tools(&["openbot.remember".to_string()])
        .expect("describe_tools");
    assert_eq!(described.tools.len(), 1);

    // Recursive meta-tool refusal
    let meta_err = host.tool_router().call_tool(
        &ToolCallContext::for_agent(&a1.id),
        "openbot.tool_search",
        r#"{"query":"browser"}"#,
        false,
    );
    assert!(matches!(meta_err, Err(OpenBotRuntimeError::Validation(_))));

    // [null] structured item guard
    let null_err = host.tool_router().call_tool(
        &ToolCallContext::for_agent(&a1.id),
        "openbot.ask_user",
        r#"{"questions":[null]}"#,
        false,
    );
    assert!(matches!(null_err, Err(OpenBotRuntimeError::Validation(_))));

    // JSON escape repair returning repaired_json = true
    let repaired = host
        .tool_router()
        .call_tool(
            &ToolCallContext::for_agent(&a1.id),
            "openbot.remember",
            r#"{"text":"Check C:\Users\demo\notes.txt"}"#,
            true,
        )
        .expect("repaired json call");
    assert!(repaired.repaired_json);

    // 2. 3-Tier Memory (essential <= 4096 B, FTS5 bm25 + CJK, site recall) + isolated channel memory isolation
    let mem_cjk = host
        .tool_router()
        .memory_put(MemoryPutInput {
            agent_id: a1.id.clone(),
            text: "プロジェクトのAPI設定を本番向けに固定する".to_string(),
            inclusion: Some(MemoryInclusion::Essential),
            site: Some("https://www.docs.example.com/guide".to_string()),
            ..Default::default()
        })
        .expect("memory_put CJK");
    assert_eq!(mem_cjk.inclusion, MemoryInclusion::Essential);
    assert!(mem_cjk.essential_bytes <= 4_096);

    let mem_site = host
        .tool_router()
        .memory_put(MemoryPutInput {
            agent_id: a1.id.clone(),
            text: "Use pagination cursor on billing portal".to_string(),
            inclusion: Some(MemoryInclusion::Searchable),
            site: Some("https://portal.example.com/invoices".to_string()),
            ..Default::default()
        })
        .expect("memory_put site");
    assert_eq!(mem_site.inclusion, MemoryInclusion::Searchable);

    let cjk_hits = host
        .tool_router()
        .memory_search(MemorySearchInput {
            agent_id: a1.id.clone(),
            query: Some("設定".to_string()),
            host: Some("portal.example.com".to_string()),
            limit: Some(5),
            channel_id: None,
        })
        .expect("memory_search CJK + host");
    assert!(!cjk_hits.memories.is_empty());
    assert!(!cjk_hits.site_memories.is_empty());

    // Isolated channel memory separation
    let iso_ch = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Isolated Secret Room".to_string(),
            title: "Isolated".to_string(),
            instructions: "Isolated memory".to_string(),
            members: vec![ChannelMember { agent_id: a1.id.clone() }],
            lead_agent_id: Some(a1.id.clone()),
            isolated: Some(true),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create isolated channel");
    let iso_ctx = ToolCallContext::for_channel(&a1.id, &iso_ch.id, None);
    let ch_mem = host
        .tool_router()
        .call_tool_value(
            &iso_ctx,
            "openbot.channel_remember",
            json!({ "channelId": iso_ch.id, "text": "ISOLATED_CHANNEL_ONLY_FACT" }),
        )
        .expect("channel_remember");
    assert_eq!(ch_mem.result.get("accepted").and_then(Value::as_bool), Some(true));
    // Private agent memory search must be forbidden from inside isolated channel
    let leak_attempt = host.tool_router().call_tool_value(
        &iso_ctx,
        "openbot.search_memories",
        json!({ "query": "API" }),
    );
    assert!(matches!(leak_attempt, Err(OpenBotRuntimeError::Forbidden(_))));
    // Channel memory must never appear in private agent memory search
    let priv_search = host
        .tool_router()
        .memory_search(MemorySearchInput {
            agent_id: a1.id.clone(),
            query: Some("ISOLATED_CHANNEL_ONLY_FACT".to_string()),
            ..Default::default()
        })
        .expect("private search");
    assert!(priv_search.memories.is_empty());

    // 3. SharedAgentDataDb SQL tools + owner_agent_id authorizer enforcement
    let _ = host
        .tool_router()
        .agent_data_exec(
            &a1.id,
            AgentSqlMode::Write,
            "CREATE TABLE sprint_metrics (id INTEGER PRIMARY KEY, score INTEGER)",
            None,
        )
        .expect("create table as a1");
    let _ = host
        .tool_router()
        .agent_data_exec(
            &a1.id,
            AgentSqlMode::Write,
            "INSERT INTO sprint_metrics (score) VALUES (?)",
            Some(&json!([99])),
        )
        .expect("insert row");
    let q_res = host
        .tool_router()
        .agent_data_exec(&a2.id, AgentSqlMode::Read, "SELECT score FROM sprint_metrics", None)
        .expect("a2 read allowed");
    assert_eq!(q_res["rows"][0][0], json!(99));
    let drop_by_other = host.tool_router().agent_data_exec(
        &a2.id,
        AgentSqlMode::Write,
        "DROP TABLE sprint_metrics",
        None,
    );
    assert!(drop_by_other.is_err(), "non-owner DROP TABLE must be denied by SQLite authorizer");
}

#[test]
fn ob_f10_skills_mcp_vault_and_browser_security_guards() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a1 = make_agent(&host, "SecBot");
    let cua_bot = host
        .agents()
        .create_agent_full(CreateAgentRequest {
            name: "CuaBot".to_string(),
            provider: Some(AgentProviderId::Codex),
            computer_use: Some(true),
            ..Default::default()
        })
        .expect("cua bot");

    // 1. SkillService: SKILL.md + <= 32 cap
    for i in 0..32 {
        let out = host
            .skills()
            .save_skill(SaveSkillInput {
                agent_id: a1.id.clone(),
                slug: format!("skill-{i:02}"),
                name: format!("Skill {i:02}"),
                description: format!("Automation skill #{i}"),
                instructions: format!("# Skill {i}\nRun check {i}."),
                triggers: Vec::new(),
                tools: Vec::new(),
                origin: Some("local".to_string()),
                enabled: Some(true),
            })
            .expect("save skill <= 32");
        assert_eq!(out.assigned_skill_count, i + 1);
    }
    let skill_33 = host.skills().save_skill(SaveSkillInput {
        agent_id: a1.id.clone(),
        slug: "skill-33".to_string(),
        name: "Skill 33".to_string(),
        description: "Over cap".to_string(),
        instructions: "# Too many".to_string(),
        triggers: Vec::new(),
        tools: Vec::new(),
        origin: Some("local".to_string()),
        enabled: Some(true),
    });
    assert!(matches!(skill_33, Err(OpenBotRuntimeError::LimitExceeded(_))));

    // 2. McpGateway: reserved name refusal, PKCE S256, computer_use gating, <= 100 cap
    let pkce = generate_pkce_s256().expect("pkce");
    assert!(verify_pkce_s256(&pkce.code_verifier, &pkce.code_challenge));
    assert!(!verify_pkce_s256("wrong-verifier", &pkce.code_challenge));
    assert!(host.mcp().computer_use_descriptor(&a1).is_none());
    assert!(host.mcp().computer_use_descriptor(&cua_bot).is_some());

    let reserved_mcp = host.mcp().register_server(RegisterMcpServerInput {
        id: None,
        name: "openbot_browser".to_string(),
        transport: McpTransport::Http,
        enabled: Some(true),
        command: None,
        args: Vec::new(),
        env: Vec::new(),
        env_passthrough: Vec::new(),
        working_directory: None,
        url: Some("https://mcp.example.com".to_string()),
        headers: Vec::new(),
    });
    assert!(reserved_mcp.is_err());

    for i in 0..100 {
        host.mcp()
            .register_server(RegisterMcpServerInput {
                id: None,
                name: format!("srv_{i:03}"),
                transport: McpTransport::Http,
                enabled: Some(true),
                command: None,
                args: Vec::new(),
                env: Vec::new(),
                env_passthrough: Vec::new(),
                working_directory: None,
                url: Some(format!("https://mcp{i}.example.com")),
                headers: Vec::new(),
            })
            .expect("register mcp <= 100");
    }
    let mcp_101 = host.mcp().register_server(RegisterMcpServerInput {
        id: None,
        name: "srv_101".to_string(),
        transport: McpTransport::Http,
        enabled: Some(true),
        command: None,
        args: Vec::new(),
        env: Vec::new(),
        env_passthrough: Vec::new(),
        working_directory: None,
        url: Some("https://mcp101.example.com".to_string()),
        headers: Vec::new(),
    });
    assert!(matches!(mcp_101, Err(OpenBotRuntimeError::LimitExceeded(_))));

    // 3. VaultService: ChaCha20Poly1305 encryption, 0600 API key, RFC 6238 TOTP, Link payment
    let saved_login = host
        .vault()
        .save_item(
            VaultSaveInput::Login {
                id: None,
                website: "https://app.example.com".to_string(),
                username: "alice@example.com".to_string(),
                notes: "prod login".to_string(),
                password: Some("SuperSecretVaultPass!2026".to_string()),
                two_step_key: Some("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".to_string()),
                agents: Some(vec![a1.id.clone()]),
            },
            Some(&a1.id),
        )
        .expect("save vault login");
    let vault_raw = fs::read_to_string(host.vault().vault_path()).expect("read vault file");
    assert!(!vault_raw.contains("SuperSecretVaultPass!2026"));

    let totp = compute_totp("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", 59, 30, 8).expect("rfc6238");
    assert_eq!(totp, "94287082");

    let api_file = host
        .vault()
        .save_api_key_file("STRIPE_TEST_KEY", "rk_test_000111222", Some(&a1.id))
        .expect("save api key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&api_file.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let long_ctx = "Purchasing annual CI runner quota for team openbot build pipeline verification and automated release signing infrastructure.".to_string();
    assert!(long_ctx.len() >= 100);
    assert!(
        host.vault()
            .request_link_payment(LinkPaymentRequest {
                amount: 2500,
                currency: Some("USD".to_string()),
                merchant_name: "Cloud Runner".to_string(),
                merchant_url: "http://insecure.example.com/pay".to_string(),
                context: long_ctx.clone(),
                approved: true,
                agent_id: Some(a1.id.clone()),
            })
            .is_err()
    );
    assert!(
        host.vault()
            .request_link_payment(LinkPaymentRequest {
                amount: 2500,
                currency: Some("USD".to_string()),
                merchant_name: "Cloud Runner".to_string(),
                merchant_url: "https://secure.example.com/pay".to_string(),
                context: "too short".to_string(),
                approved: true,
                agent_id: Some(a1.id.clone()),
            })
            .is_err()
    );
    let link_card = host
        .vault()
        .request_link_payment(LinkPaymentRequest {
            amount: 2500,
            currency: Some("USD".to_string()),
            merchant_name: "Cloud Runner".to_string(),
            merchant_url: "https://secure.example.com/pay".to_string(),
            context: long_ctx,
            approved: true,
            agent_id: Some(a1.id.clone()),
        })
        .expect("request_link_payment");
    assert_eq!(link_card.last4, "4242");
    assert_eq!(link_card.masked_number, "**** **** **** 4242");

    // 4. BrowserService: fill_fields secret refusal, submit_secret, fill_card HTTPS + screenshots_blocked,
    //    request_takeover pause/release, <=200 steps, <=25 tabs
    let tab = host
        .browser()
        .open_tab("https://app.example.com/login", Some(&a1.id), None, None, false)
        .expect("open tab");

    let secret_fill_err = host.browser().fill_fields(FillFieldsRequest {
        tab_id: tab.tab_id.clone(),
        revision: None,
        fields: vec![FillFieldEntry {
            r#ref: None,
            selector: Some("input[type=password]".to_string()),
            name: Some("password".to_string()),
            label: Some("Password".to_string()),
            text: "my-password-123".to_string(),
        }],
        timeout_ms: None,
    });
    assert!(matches!(secret_fill_err, Err(OpenBotRuntimeError::Forbidden(_))));

    let submitted = host
        .browser()
        .submit_secret(SubmitSecretRequest {
            tab_id: tab.tab_id.clone(),
            method: "password".to_string(),
            targets: vec![BrowserTarget::Css {
                selector: "input[type=password]".to_string(),
                nth: None,
            }],
            digits: None,
            submission: "enter".to_string(),
            submit_target: None,
            login_id: Some(saved_login.id().to_string()),
            username: Some("alice@example.com".to_string()),
            agent_id: Some(a1.id.clone()),
        })
        .expect("submit_secret");
    assert!(submitted.secret_document);
    assert!(!serde_json::to_string(&submitted).unwrap().contains("SuperSecretVaultPass!2026"));

    let _ = host
        .browser()
        .navigate(&tab.tab_id, Some("https://secure.example.com/pay"), None)
        .expect("nav to pay");
    let card_res = host
        .browser()
        .fill_card(FillCardRequest {
            tab_id: tab.tab_id.clone(),
            card_id: link_card.card_id.clone(),
            targets: None,
            approved: true,
            agent_id: Some(a1.id.clone()),
            timeout_ms: None,
        })
        .expect("fill_card");
    assert!(card_res.filled_card && card_res.screenshots_blocked);
    assert!(matches!(
        host.browser().screenshot(&tab.tab_id, false),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));
    assert!(matches!(
        host.browser().evaluate(&tab.tab_id, "document.title"),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));
    assert!(matches!(
        host.browser().recording_start(&tab.tab_id),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));

    let tk = host
        .browser()
        .request_takeover(TakeoverRequest {
            tab_id: tab.tab_id.clone(),
            reason: Some("captcha".to_string()),
            agent_id: Some(a1.id.clone()),
        })
        .expect("request_takeover");
    assert!(tk.takeover_active);
    assert!(host.browser().scroll(&tab.tab_id, 0.0, 200.0).is_err());
    let released = host
        .browser()
        .release_takeover(&tab.tab_id, Some(&tk.takeover_id))
        .expect("release_takeover");
    assert!(!released.takeover_active);

    let too_many_steps: Vec<Value> = (0..201).map(|_| json!({"action":"snapshot"})).collect();
    let script_err = host.browser().run_script_steps(BrowserRunStepsRequest {
        agent_id: Some(a1.id.clone()),
        tab_id: Some(tab.tab_id.clone()),
        url: None,
        steps: too_many_steps,
        timeout_ms: None,
        final_snapshot: Some(false),
    });
    assert!(matches!(script_err, Err(OpenBotRuntimeError::LimitExceeded(_))));

    for i in 1..MAX_BROWSER_TABS {
        host.browser()
            .open_tab(&format!("https://tab{i}.example.com"), Some(&a1.id), None, None, false)
            .expect("open tab <= 25");
    }
    let tab_26 = host
        .browser()
        .open_tab("https://tab26.example.com", Some(&a1.id), None, None, false);
    assert!(matches!(tab_26, Err(OpenBotRuntimeError::LimitExceeded(_))));
}

#[test]
fn m3_r2_regression_catalog_schema_aligned_router_tools() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let owner = make_agent(&host, "SchemaOwner");
    let mut user_ctx = ToolCallContext::for_agent(&owner.id);
    user_ctx.user_initiated = true;

    // Defect 1: openbot.save_api_key accepts catalog-required "key" field (plus legacy "value")
    let saved_key = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.save_api_key",
            json!({ "name": "DEMO_KEY", "key": "sk-demo-123", "notes": "catalog schema" }),
        )
        .expect("save_api_key with catalog 'key' param");
    assert_eq!(saved_key.result["name"], json!("DEMO_KEY"));
    let listed_keys = host
        .tool_router()
        .call_tool_value(&user_ctx, "openbot.list_api_keys", json!({}))
        .expect("list_api_keys");
    assert!(
        listed_keys.result["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["name"] == json!("DEMO_KEY"))
    );

    // Defect 2.1: openbot.create_agent supports catalog minimal {"name": "QuickBot"} and firstTask
    let quick = host
        .tool_router()
        .call_tool_value(&user_ctx, "openbot.create_agent", json!({ "name": "QuickBot" }))
        .expect("create_agent minimal catalog payload");
    let quick_id = quick.result["id"].as_str().expect("quick id").to_string();
    assert_eq!(quick.result["name"], json!("QuickBot"));

    let tasked = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.create_agent",
            json!({ "name": "TaskedBot", "firstTask": "Draft Q4 roadmap" }),
        )
        .expect("create_agent with firstTask");
    let tasked_id = tasked.result["id"].as_str().expect("tasked id").to_string();

    // Creator privilege inheritance & widening refusal on agent-initiated create_agent
    let agent_ctx = ToolCallContext::for_agent(&owner.id);
    let widen_create = host.tool_router().call_tool_value(
        &agent_ctx,
        "openbot.create_agent",
        json!({ "name": "TooPrivileged", "access": "full" }),
    );
    assert!(matches!(widen_create, Err(OpenBotRuntimeError::Forbidden(_))));
    let inherited = host
        .tool_router()
        .call_tool_value(&agent_ctx, "openbot.create_agent", json!({ "name": "ChildBot" }))
        .expect("agent create_agent inherits limits");
    let child_id = inherited.result["id"].as_str().unwrap().to_string();
    assert_eq!(inherited.result["access"], json!("workspace"));
    assert_eq!(inherited.result["computerUse"], json!(false));

    // Defect 2.2: openbot.update_agents supports catalog {"agentIds": [...], ...} + legacy "updates"
    let batch_upd = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.update_agents",
            json!({
                "agentIds": [&quick_id, &tasked_id],
                "model": "gpt-5.4",
                "notifications": false
            }),
        )
        .expect("update_agents with catalog agentIds");
    let updated_arr = batch_upd.result["agents"].as_array().expect("agents array");
    assert_eq!(updated_arr.len(), 2);
    assert!(updated_arr.iter().all(|a| a["model"] == json!("gpt-5.4")));
    assert!(updated_arr.iter().all(|a| a["notifications"] == json!(false)));

    let legacy_upd = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.update_agents",
            json!({ "updates": [{ "agentId": &child_id, "notifications": false }] }),
        )
        .expect("update_agents legacy updates array");
    assert_eq!(legacy_upd.result["agents"][0]["notifications"], json!(false));

    // Defect 2.3: openbot.delete_agent supports catalog {"agentIds": [...]} + approval gate + self guard
    let self_del = host.tool_router().call_tool_value(
        &agent_ctx,
        "openbot.delete_agent",
        json!({ "agentIds": [&owner.id] }),
    );
    assert!(matches!(self_del, Err(OpenBotRuntimeError::Forbidden(_))));

    let approval_del = host.tool_router().call_tool_value(
        &agent_ctx,
        "openbot.delete_agent",
        json!({ "agentIds": [&quick_id, &tasked_id] }),
    );
    assert!(matches!(approval_del, Err(OpenBotRuntimeError::ApprovalRequired { .. })));

    let del_batch = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.delete_agent",
            json!({ "agentIds": [&quick_id, &tasked_id] }),
        )
        .expect("delete_agent with catalog agentIds");
    assert_eq!(del_batch.result["deleted"], json!(true));
    assert_eq!(del_batch.result["deletedCount"], json!(2));
    assert!(host.agents().get_agent(&quick_id).is_none());
    assert!(host.agents().get_agent(&tasked_id).is_none());

    let del_single = host
        .tool_router()
        .call_tool_value(&user_ctx, "openbot.delete_agent", json!({ "agentId": &child_id }))
        .expect("delete_agent singular backward compat");
    assert_eq!(del_single.result["deleted"], json!(true));
    assert!(host.agents().get_agent(&child_id).is_none());

    // Defect 2.4: openbot.start_task accepts catalog-required "brief" and enqueues it onto the side task
    let started_task = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.start_task",
            json!({ "title": "Release Audit", "brief": "Verify all migration checksums" }),
        )
        .expect("start_task with catalog brief");
    let task_id = started_task.result["taskId"].as_str().expect("taskId");
    let side_tasks = host.scheduler().list_side_tasks(&owner.id).expect("list_side_tasks");
    let created_side = side_tasks
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .expect("created side task");
    assert!(
        created_side
            .queued_messages
            .iter()
            .any(|m| m.text.contains("Verify all migration checksums")),
        "start_task brief must be enqueued on the side task"
    );

    // Defect 2.5: openbot.pin_message honors catalog "pinned": false (and optional "quote")
    let sent = host
        .scheduler()
        .send_message(SendChatMessageInput {
            agent_id: owner.id.clone(),
            text: "Critical release freeze at 18:00 UTC".to_string(),
            ..Default::default()
        })
        .expect("send_message");
    let pinned_res = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.pin_message",
            json!({ "messageId": &sent.message_id, "pinned": true }),
        )
        .expect("pin with pinned: true");
    assert_eq!(pinned_res.result["pinned"], json!(true));
    assert_eq!(host.scheduler().list_pins(&owner.id).unwrap().pins.len(), 1);

    let unpinned_res = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.pin_message",
            json!({ "messageId": &sent.message_id, "pinned": false }),
        )
        .expect("unpin with pinned: false");
    assert_eq!(unpinned_res.result["pinned"], json!(false));
    assert!(host.scheduler().list_pins(&owner.id).unwrap().pins.is_empty());

    let pinned_by_quote = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.pin_message",
            json!({ "quote": "release freeze at 18:00", "pinned": true }),
        )
        .expect("pin by quote");
    assert_eq!(pinned_by_quote.result["pinned"], json!(true));
    let unpinned_by_quote = host
        .tool_router()
        .call_tool_value(
            &user_ctx,
            "openbot.pin_message",
            json!({ "quote": "release freeze at 18:00", "pinned": false }),
        )
        .expect("unpin by quote");
    assert_eq!(unpinned_by_quote.result["pinned"], json!(false));
    assert!(host.scheduler().list_pins(&owner.id).unwrap().pins.is_empty());
}

#[test]
fn m3_r2_regression_router_aliases_and_extended_browser_tools() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a1 = make_agent(&host, "AliasRunner");
    let a2 = make_agent(&host, "AliasPeer");
    let ctx = ToolCallContext::for_agent(&a1.id);

    // ToolCatalog count remains strictly 160
    assert_eq!(host.tool_router().catalog().all_tools().len(), FULL_CATALOG_COUNT);

    // Genuinely unknown openbot.* and openbot_browser.* tools must still return NotFound
    assert!(matches!(
        host.tool_router().call_tool_value(&ctx, "openbot.does_not_exist", json!({})),
        Err(OpenBotRuntimeError::NotFound(_))
    ));
    assert!(matches!(
        host.tool_router()
            .call_tool_value(&ctx, "openbot_browser.does_not_exist", json!({})),
        Err(OpenBotRuntimeError::NotFound(_))
    ));
    assert!(matches!(
        host.tool_router()
            .call_tool_value(&ctx, "openbot.tool_call", json!({ "name": "openbot.list_agents", "arguments": {} })),
        Err(OpenBotRuntimeError::Validation(_))
    ));

    // Memory aliases (including raw JSON escape repair)
    let put = host
        .tool_router()
        .call_tool(
            &ctx,
            "memory_put",
            r#"{"text":"Alias path C:\Users\demo\spec.md", }"#,
            true,
        )
        .expect("memory_put alias with JSON repair");
    assert!(put.repaired_json);
    let mem_id = put.result["memory"]["id"].as_str().expect("memory id").to_string();

    let saved = host
        .tool_router()
        .call_tool_value(&ctx, "memory_save", json!({ "text": "Second alias memory" }))
        .expect("memory_save alias");
    assert!(saved.result["memory"]["id"].is_string());

    let ess = host
        .tool_router()
        .call_tool_value(&ctx, "essential_memory", json!({ "text": "Essential alias note" }))
        .expect("essential_memory alias");
    assert_eq!(ess.result["inclusion"], json!("essential"));

    let found = host
        .tool_router()
        .call_tool_value(&ctx, "memory_search", json!({ "query": "spec" }))
        .expect("memory_search alias");
    assert!(!found.result["memories"].as_array().unwrap().is_empty());

    let deleted = host
        .tool_router()
        .call_tool_value(&ctx, "memory_delete", json!({ "memoryId": &mem_id }))
        .expect("memory_delete alias");
    assert_eq!(deleted.result["deleted"], json!(true));

    // SQL aliases
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "sql_exec",
            json!({ "sql": "CREATE TABLE alias_items (id INTEGER PRIMARY KEY, label TEXT)" }),
        )
        .expect("sql_exec alias");
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "agent_data_exec",
            json!({ "mode": "write", "sql": "INSERT INTO alias_items (label) VALUES (?)", "params": ["ok"] }),
        )
        .expect("agent_data_exec alias");
    let q = host
        .tool_router()
        .call_tool_value(&ctx, "sql_query", json!({ "sql": "SELECT label FROM alias_items" }))
        .expect("sql_query alias");
    assert_eq!(q.result["rows"][0][0], json!("ok"));
    let tbls = host
        .tool_router()
        .call_tool_value(&ctx, "sql_list_tables", json!({}))
        .expect("sql_list_tables alias");
    assert_eq!(tbls.result["count"], json!(1));

    // Channel aliases
    let ch = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Alias Room".to_string(),
            title: "Initial Topic".to_string(),
            instructions: "Test aliases".to_string(),
            members: vec![
                ChannelMember { agent_id: a1.id.clone() },
                ChannelMember { agent_id: a2.id.clone() },
            ],
            lead_agent_id: Some(a1.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create channel");
    let post = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: ch.id.clone(),
            text: format!("@[AliasRunner](agent:{}) check alias tools", a1.id),
            hold_running: true,
            ..Default::default()
        })
        .expect("post person request");
    let task_id = post.tasks[0].task.id.clone();
    let ch_ctx = ToolCallContext::for_channel(&a1.id, &ch.id, Some(task_id.clone()));

    let status = host
        .tool_router()
        .call_tool(&ch_ctx, "channel_status", r#"{ }"#, true)
        .expect("channel_status alias");
    assert_eq!(status.result["channel"]["id"], json!(ch.id));

    let members = host
        .tool_router()
        .call_tool_value(&ch_ctx, "channel_list_members", json!({}))
        .expect("channel_list_members alias");
    assert_eq!(members.result["members"].as_array().unwrap().len(), 2);

    let awaited = host
        .tool_router()
        .call_tool_value(&ch_ctx, "channel_await", json!({}))
        .expect("channel_await alias");
    assert_eq!(awaited.result["allSettled"], json!(true));

    let topic = host
        .tool_router()
        .call_tool_value(&ch_ctx, "channel_set_topic", json!({ "topic": "Updated Topic" }))
        .expect("channel_set_topic alias");
    assert_eq!(topic.result["accepted"], json!(true));

    let ch_mem = host
        .tool_router()
        .call_tool_value(&ch_ctx, "openbot.channel_remember", json!({ "text": "Forgettable note" }))
        .expect("channel_remember");
    let ch_mem_id = ch_mem.result["memory"]["id"].as_str().unwrap().to_string();
    let forgot = host
        .tool_router()
        .call_tool_value(&ch_ctx, "channel_forget", json!({ "memoryId": &ch_mem_id }))
        .expect("channel_forget alias");
    assert_eq!(forgot.result["accepted"], json!(true));

    let _ = host
        .tool_router()
        .call_tool_value(&ch_ctx, "channel_send", json!({ "text": "Follow-up ping" }))
        .expect("channel_send alias");

    let _ = host.channels().stop_task(&ch.id, &task_id, None).expect("stop task");
    let reass = host
        .tool_router()
        .call_tool_value(
            &ch_ctx,
            "channel_reassign",
            json!({ "taskId": &task_id, "recipientAgentId": &a2.id }),
        )
        .expect("channel_reassign alias");
    assert_eq!(reass.result["accepted"], json!(true));
    let _ = host.channels().stop_task(&ch.id, &task_id, None).expect("stop reassigned");
    let cont = host
        .tool_router()
        .call_tool_value(&ch_ctx, "channel_continue", json!({ "taskId": &task_id }))
        .expect("channel_continue alias");
    assert_eq!(cont.result["accepted"], json!(true));

    // All 16 extended openbot_browser.* convenience tools reachable via call_tool
    let tab1 = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.new_tab", json!({ "url": "https://one.example.com" }))
        .expect("browser.new_tab");
    let tab1_id = tab1.result["tabId"].as_str().expect("tab1 id").to_string();
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.navigate",
            json!({ "tabId": &tab1_id, "url": "https://two.example.com" }),
        )
        .expect("navigate");
    let tab2 = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.new_tab", json!({ "url": "https://three.example.com" }))
        .expect("browser.new_tab 2");
    let tab2_id = tab2.result["tabId"].as_str().expect("tab2 id").to_string();

    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.switch_tab", json!({ "tabId": &tab1_id }))
        .expect("browser.switch_tab");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.back", json!({ "tabId": &tab1_id }))
        .expect("browser.back");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.forward", json!({ "tabId": &tab1_id }))
        .expect("browser.forward");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.reload", json!({ "tabId": &tab1_id }))
        .expect("browser.reload");
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.double_click",
            json!({ "tabId": &tab1_id, "target": { "kind": "css", "selector": "#cta" } }),
        )
        .expect("browser.double_click");
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.press_key",
            json!({ "tabId": &tab1_id, "key": "Enter" }),
        )
        .expect("browser.press_key");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.extract_text", json!({ "tabId": &tab1_id }))
        .expect("browser.extract_text");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.get_links", json!({ "tabId": &tab1_id }))
        .expect("browser.get_links");
    let _ = host
        .tool_router()
        .call_tool(&ctx, "openbot_browser.console_logs", &format!(r#"{{"tabId":"{tab1_id}","clear":"false", }}"#), true)
        .expect("browser.console_logs with JSON repair");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.network_log", json!({ "tabId": &tab1_id }))
        .expect("browser.network_log");
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.download",
            json!({ "tabId": &tab1_id, "url": "https://two.example.com/report.csv" }),
        )
        .expect("browser.download");
    let upload_fixture = dir.path().join("sample-upload.txt");
    fs::write(&upload_fixture, "upload payload").expect("write upload fixture");
    let _ = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.upload_file",
            json!({
                "tabId": &tab1_id,
                "target": { "kind": "css", "selector": "input[type=file]" },
                "path": upload_fixture.to_string_lossy()
            }),
        )
        .expect("browser.upload_file");
    let _ = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.cookies", json!({ "action": "list" }))
        .expect("browser.cookies");
    let tk = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.request_takeover",
            json!({ "tabId": &tab1_id, "reason": "mfa" }),
        )
        .expect("browser.request_takeover");
    let rel = host
        .tool_router()
        .call_tool_value(
            &ctx,
            "openbot_browser.release_takeover",
            json!({ "tabId": &tab1_id, "takeoverId": tk.result["takeoverId"] }),
        )
        .expect("browser.release_takeover");
    assert_eq!(rel.result["takeoverActive"], json!(false));
    let closed = host
        .tool_router()
        .call_tool_value(&ctx, "openbot_browser.close", json!({ "tabId": &tab2_id }))
        .expect("browser.close");
    assert_eq!(closed.result["closedCount"], json!(1));
}

#[test]
fn m3_r2_regression_queued_channel_reply_blocks_premature_channel_result() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let alice = make_agent(&host, "Alice");
    let bob = make_agent(&host, "Bob");

    let ch = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Reply Guard Room".to_string(),
            title: "OB-F09 reply test".to_string(),
            instructions: "Coordinate".to_string(),
            members: vec![
                ChannelMember { agent_id: alice.id.clone() },
                ChannelMember { agent_id: bob.id.clone() },
            ],
            lead_agent_id: Some(alice.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create channel");

    // Scenario A: Queued peer channel_reply blocks premature channel_result until answered
    let post_a = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: ch.id.clone(),
            text: "@everyone verify deployment".to_string(),
            hold_running: true,
            ..Default::default()
        })
        .expect("post A");
    let t_alice = post_a
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(alice.id.as_str()))
        .unwrap()
        .task
        .clone();
    let t_bob = post_a
        .tasks
        .iter()
        .find(|t| t.task.owner_agent_id.as_deref() == Some(bob.id.as_str()))
        .unwrap()
        .task
        .clone();

    let res_alice = host
        .channels()
        .result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: t_alice.id.clone(),
            agent_id: alice.id.clone(),
            text: "Build hash is abc123.".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("alice result");

    let queued_reply = host
        .channels()
        .reply(ChannelReplyInput {
            channel_id: ch.id.clone(),
            task_id: t_bob.id.clone(),
            agent_id: bob.id.clone(),
            message_id: res_alice.message_id.clone(),
            text: "Alice, does abc123 include the hotfix?".to_string(),
            turn_id: None,
            simulate_peer_failure: false,
            operation_id: None,
        })
        .expect("queued channel_reply");
    assert_eq!(queued_reply.task.state, ChannelTaskState::Queued);
    assert!(!queued_reply.caller_resumed);

    // Premature channel_result by Bob MUST be blocked with Conflict while Alice's reply is Queued
    let premature = host.channels().result(ChannelResultInput {
        channel_id: ch.id.clone(),
        task_id: t_bob.id.clone(),
        agent_id: bob.id.clone(),
        text: "Premature final answer".to_string(),
        turn_id: None,
        operation_id: None,
    });
    assert!(
        matches!(premature, Err(OpenBotRuntimeError::Conflict(_))),
        "channel_result must be rejected while peer channel_reply task is Queued, got: {premature:?}"
    );

    // Once Alice answers the reply task, Bob's task is released and Bob can publish channel_result
    let peer_answer = host
        .channels()
        .result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: queued_reply.reply_task_id.clone(),
            agent_id: alice.id.clone(),
            text: "Yes, hotfix is included.".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("alice answers reply");
    assert_eq!(peer_answer.released_parent_id.as_deref(), Some(t_bob.id.as_str()));

    let bob_done = host
        .channels()
        .result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: t_bob.id.clone(),
            agent_id: bob.id.clone(),
            text: "Confirmed hotfix in abc123.".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("bob result allowed after reply answered");
    assert!(bob_done.accepted);

    // Scenario B: Failed reply task settles dependency so caller is NOT deadlocked
    let post_b = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: ch.id.clone(),
            text: format!("@[Bob](agent:{}) re-check staging", bob.id),
            hold_running: true,
            ..Default::default()
        })
        .expect("post B");
    let t_bob_2 = post_b.tasks[0].task.clone();
    let queued_reply_2 = host
        .channels()
        .reply(ChannelReplyInput {
            channel_id: ch.id.clone(),
            task_id: t_bob_2.id.clone(),
            agent_id: bob.id.clone(),
            message_id: res_alice.message_id.clone(),
            text: "Alice, is staging healthy?".to_string(),
            turn_id: None,
            simulate_peer_failure: false,
            operation_id: None,
        })
        .expect("second queued reply");
    assert!(matches!(
        host.channels().result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: t_bob_2.id.clone(),
            agent_id: bob.id.clone(),
            text: "Still premature".to_string(),
            turn_id: None,
            operation_id: None,
        }),
        Err(OpenBotRuntimeError::Conflict(_))
    ));
    let released_on_fail = host
        .channels()
        .fail_task(&ch.id, &queued_reply_2.reply_task_id, "peer timeout")
        .expect("fail reply task");
    assert_eq!(released_on_fail.as_ref().map(|t| t.id.as_str()), Some(t_bob_2.id.as_str()));
    let bob_after_fail = host
        .channels()
        .result(ChannelResultInput {
            channel_id: ch.id.clone(),
            task_id: t_bob_2.id.clone(),
            agent_id: bob.id.clone(),
            text: "Proceeding after Alice timed out.".to_string(),
            turn_id: None,
            operation_id: None,
        })
        .expect("failed reply must settle dependency to prevent deadlock");
    assert!(bob_after_fail.accepted);
}

#[test]
fn m3_r2_regression_isolated_channel_remember_redirects_without_private_leak() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let alice = make_agent(&host, "IsolatedAlice");

    let private_mem = host
        .tool_router()
        .call_tool_value(
            &ToolCallContext::for_agent(&alice.id),
            "openbot.remember",
            json!({ "text": "PRIVATE_AGENT_BASELINE_FACT" }),
        )
        .expect("baseline private memory");
    let private_mem_id = private_mem.result["memory"]["id"]
        .as_str()
        .expect("private mem id")
        .to_string();

    let iso_ch = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Isolated Sandbox".to_string(),
            title: "Isolated".to_string(),
            instructions: "Never leak".to_string(),
            members: vec![ChannelMember { agent_id: alice.id.clone() }],
            lead_agent_id: Some(alice.id.clone()),
            isolated: Some(true),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create isolated channel");

    let iso_ctx = ToolCallContext::for_channel(&alice.id, &iso_ch.id, None);

    // openbot.remember + aliases redirect cleanly to ChannelMemoryStore with savedAs: "channel memory"
    for (tool, fact) in [
        ("openbot.remember", "ISO_REDIRECT_FACT_PRIMARY"),
        ("memory_save", "ISO_REDIRECT_FACT_SAVE"),
        ("memory_put", "ISO_REDIRECT_FACT_PUT"),
        ("essential_memory", "ISO_REDIRECT_FACT_ESSENTIAL"),
    ] {
        let out = host
            .tool_router()
            .call_tool_value(&iso_ctx, tool, json!({ "text": fact }))
            .expect("isolated remember redirect");
        assert_eq!(out.result["status"], json!("saved"));
        assert_eq!(out.result["savedAs"], json!("channel memory"));
        assert!(out.result["memory"]["id"].is_string());
    }

    let channel_memories = host
        .database()
        .channel_memory_store()
        .list(&iso_ch.id)
        .expect("list channel memories");
    assert_eq!(channel_memories.len(), 4);
    assert!(channel_memories.iter().any(|m| m.entry.text == "ISO_REDIRECT_FACT_PRIMARY"));

    // None of the redirected facts may leak into private AgentMemoryStore
    assert_eq!(
        host.database()
            .agent_memory_store()
            .count(&alice.id)
            .expect("private memory count"),
        1
    );
    let priv_search = host
        .tool_router()
        .memory_search(MemorySearchInput {
            agent_id: alice.id.clone(),
            query: Some("ISO_REDIRECT".to_string()),
            ..Default::default()
        })
        .expect("private search");
    assert!(priv_search.memories.is_empty());

    // Passing memoryId to openbot.remember in an isolated channel is forbidden (cannot mutate private memory)
    assert!(matches!(
        host.tool_router().call_tool_value(
            &iso_ctx,
            "openbot.remember",
            json!({ "memoryId": &private_mem_id, "text": "Attempt overwrite private" }),
        ),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));

    // All private memory read/mutate tools remain strictly Forbidden inside isolated channels
    assert!(matches!(
        host.tool_router()
            .call_tool_value(&iso_ctx, "openbot.search_memories", json!({ "query": "PRIVATE" })),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));
    assert!(matches!(
        host.tool_router()
            .call_tool_value(&iso_ctx, "openbot.list_memories", json!({})),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));
    assert!(matches!(
        host.tool_router()
            .call_tool_value(&iso_ctx, "openbot.forget_memory", json!({ "memoryId": &private_mem_id })),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));
    assert!(matches!(
        host.tool_router().call_tool_value(
            &iso_ctx,
            "openbot.set_memory_inclusion",
            json!({ "changes": [{ "memoryId": &private_mem_id, "inclusion": "essential", "expectedRevision": 0 }] }),
        ),
        Err(OpenBotRuntimeError::Forbidden(_))
    ));
}

#[test]
fn m6_t2_regression_channel_empty_create_rejected_update_and_reassign_member_rules() {
    let dir = TempDir::new().expect("tempdir");
    let host = OpenBotHostRuntime::open(dir.path()).expect("open host runtime");
    let a1 = make_agent(&host, "MemberOne");
    let a2 = make_agent(&host, "MemberTwo");
    let outsider = make_agent(&host, "OutsiderThree");

    // 1. Creating a channel with 0 members must fail with Validation mentioning empty/at least/required
    let empty_err = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "NoRoster".to_string(),
            title: String::new(),
            instructions: String::new(),
            members: vec![],
            lead_agent_id: None,
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect_err("0-member channel creation must fail");
    let OpenBotRuntimeError::Validation(empty_msg) = empty_err else {
        panic!("expected Validation error for 0-member create, got {empty_err:?}");
    };
    let empty_lower = empty_msg.to_ascii_lowercase();
    assert!(
        empty_lower.contains("at least")
            && empty_lower.contains("required")
            && empty_lower.contains("empty"),
        "unexpected empty-member error: {empty_msg}"
    );

    // 2. Updating an existing channel to 0 members AND remove_deleted_member on the last member still succeed
    let solo = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "Shrinkable".to_string(),
            title: String::new(),
            instructions: String::new(),
            members: vec![ChannelMember { agent_id: a1.id.clone() }],
            lead_agent_id: Some(a1.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create 1-member channel");

    let emptied = host
        .channels()
        .update(UpdateChannelInput {
            channel_id: solo.id.clone(),
            members: Some(vec![]),
            lead_agent_id: Some(None),
            ..Default::default()
        })
        .expect("updating existing channel to 0 members must remain allowed");
    assert!(emptied.draft.members.is_empty());

    let restored = host
        .channels()
        .update(UpdateChannelInput {
            channel_id: solo.id.clone(),
            members: Some(vec![ChannelMember { agent_id: a1.id.clone() }]),
            lead_agent_id: Some(Some(a1.id.clone())),
            ..Default::default()
        })
        .expect("re-add member");
    assert_eq!(restored.draft.members.len(), 1);

    let affected = host
        .channels()
        .remove_deleted_member(&a1.id)
        .expect("remove_deleted_member must allow dropping last member");
    assert!(affected.iter().any(|c| c.id == solo.id && c.draft.members.is_empty()));

    // 3. Reassigning a task to a non-member must fail with 'not a member' & 'channel member is required'
    let team = host
        .channels()
        .create(CreateChannelInput {
            channel_id: None,
            name: "ReassignCheck".to_string(),
            title: String::new(),
            instructions: String::new(),
            members: vec![
                ChannelMember { agent_id: a1.id.clone() },
                ChannelMember { agent_id: a2.id.clone() },
            ],
            lead_agent_id: Some(a1.id.clone()),
            isolated: Some(false),
            browser: None,
            icon: None,
            image_url: None,
            folders: None,
            operation_id: None,
        })
        .expect("create team channel");

    let posted = host
        .channels()
        .post(PostChannelMessageInput {
            channel_id: team.id.clone(),
            text: "Test reassign validation".to_string(),
            router_mock: Some(r#"{"agentIds":["m1"]}"#.to_string()),
            hold_running: true,
            ..Default::default()
        })
        .expect("post held task");
    let tid = posted.tasks[0].task.id.clone();

    let reassign_err = host
        .channels()
        .reassign(ChannelReassignInput {
            channel_id: team.id.clone(),
            task_id: tid,
            recipient_agent_id: outsider.id.clone(),
            operation_id: None,
        })
        .expect_err("reassign to non-member must fail");
    let OpenBotRuntimeError::Validation(reassign_msg) = reassign_err else {
        panic!("expected Validation for non-member reassign, got {reassign_err:?}");
    };
    let reassign_lower = reassign_msg.to_ascii_lowercase();
    assert!(
        reassign_lower.contains("not a member") && reassign_lower.contains("channel member is required"),
        "unexpected non-member reassign error: {reassign_msg}"
    );
}

