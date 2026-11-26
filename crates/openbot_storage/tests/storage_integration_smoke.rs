use std::collections::BTreeMap;
use std::fs;

use openbot_contracts::agents::{CreateAgentInput, UpdateAgentInput};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::channels::{
    ChannelBrowser, ChannelDraft, ChannelMember, ChannelMessage, ChannelMessageAuthor,
    ChannelMessageAuthorKind,
};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus, ConversationSnapshot,
};
use openbot_contracts::crm::{
    CrmActor, CrmActorKind, CrmApplyResult, CrmChange, CrmCustomerInput, CrmCustomerPatch,
    CrmPrivateField,
};
use openbot_contracts::limits::INPUT_LIMITS;
use openbot_contracts::providers::{AgentProviderId, AgentReasoningEffort};
use openbot_contracts::queue::QueueDeliveryStatus;
use openbot_contracts::routines::{RoutineIntervalUnit, RoutineSchedule};
use openbot_contracts::workflows::{
    WorkflowFieldValue, WorkflowGraph, WorkflowNode, WorkflowNodeKind, WorkflowPoint,
    WorkflowRunSource,
};
use openbot_storage::*;
use tempfile::TempDir;

#[test]
fn smoke_1_open_wal_v69_schema_objects_seeds_and_integrity() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("data").join("openbot.db");
    let db = OpenBotDatabase::open(&db_path).expect("open openbot.db");
    assert!(db_path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let journal_mode: String = db
        .with_conn(|conn| Ok(conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    let versions = db.with_conn(read_applied_versions).unwrap();
    let expected_versions: Vec<i64> = (BASELINE_SCHEMA_VERSION..=LATEST_SCHEMA_VERSION).collect();
    assert_eq!(versions.len(), 62);
    assert_eq!(versions, expected_versions);

    let (regular_tables, fts_virtual, fts_shadow, indexes, triggers) = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT type, name, COALESCE(sql, '') FROM sqlite_master
                 WHERE name != 'schema_migrations' AND name NOT LIKE 'sqlite_%'",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let (mut reg, mut fts_v, mut fts_s, mut idx, mut trg) = (0, 0, 0, 0, 0);
            for item in rows {
                let (ty, name, sql) = item?;
                match ty.as_str() {
                    "table" if name == "agent_memory_search" => {
                        assert!(sql.to_ascii_uppercase().contains("USING FTS5"));
                        fts_v += 1;
                    }
                    "table" if name.starts_with("agent_memory_search_") => fts_s += 1,
                    "table" => reg += 1,
                    "index" => idx += 1,
                    "trigger" => trg += 1,
                    other => panic!("unexpected sqlite_master object type: {other} ({name})"),
                }
            }
            Ok((reg, fts_v, fts_s, idx, trg))
        })
        .unwrap();
    assert_eq!(regular_tables, 94);
    assert_eq!(fts_virtual, 1);
    assert_eq!(fts_shadow, 4);
    assert_eq!(indexes, 95);
    assert_eq!(triggers, 3);
    assert_eq!(
        regular_tables + fts_virtual + fts_shadow + indexes + triggers,
        197
    );

    let crm_status_ids: Vec<String> = db
        .crm_store()
        .statuses()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(crm_status_ids, vec!["lead", "customer", "former"]);

    let agent_store = db.agent_store_at(tmp.path().join("OpenBot")).unwrap();
    assert!(agent_store.list().is_empty(), "must seed zero starter bots");

    db.quick_check().unwrap();
    assert!(db.check_foreign_keys().unwrap().is_empty());
}

#[test]
fn smoke_2_cqrs_dispatch_idempotency_outbox_and_thread_compaction() {
    let tmp = TempDir::new().expect("tempdir");
    let db = OpenBotDatabase::open(tmp.path().join("data").join("openbot.db")).unwrap();

    let ev = OrchestrationEventInput {
        event_id: "ev-smoke-1".to_string(),
        aggregate_type: "smoke".to_string(),
        aggregate_id: "agg-1".to_string(),
        event_type: "smoke.created".to_string(),
        occurred_at: "2026-11-26T00:00:00.000Z".to_string(),
        payload: serde_json::json!({"hello": "world"}),
    };
    let r1 = db.dispatch("cmd-smoke-1", Some("op-1"), &[ev.clone()]).unwrap();
    assert!(!r1.replayed);
    assert_eq!(r1.first_sequence, 1);
    assert_eq!(r1.last_sequence, 1);

    let r2 = db.dispatch("cmd-smoke-1", Some("op-1"), &[ev]).unwrap();
    assert!(r2.replayed);
    assert_eq!(r2.first_sequence, r1.first_sequence);
    assert_eq!(r2.last_sequence, r1.last_sequence);
    assert_eq!(r2.accepted_at, r1.accepted_at);
    assert_eq!(db.read_events_since(0, 10).unwrap().len(), 1);

    let victim = tmp.path().join("obsolete_artifact.bin");
    fs::write(&victim, b"delete-me").unwrap();
    db.with_tx(|conn| {
        OpenBotDatabase::enqueue_file_deletion(
            conn,
            "del-1",
            &victim.to_string_lossy(),
            "smoke-test",
            "2026-11-26T00:00:01.000Z",
        )
    })
    .unwrap();
    assert_eq!(db.list_pending_file_deletions().unwrap().len(), 1);
    let drain = db.drain_file_deletion_outbox().unwrap();
    assert_eq!(drain.deleted, 1);
    assert_eq!(drain.failed, 0);
    assert!(!victim.exists());
    assert!(db.list_pending_file_deletions().unwrap().is_empty());

    let agents = db.agent_store_at(tmp.path().join("OpenBot")).unwrap();
    let agent = agents
        .create_agent(&CreateAgentInput {
            name: "Compactor".to_string(),
            description: String::new(),
            avatar_seed: "compactor".to_string(),
            avatar_hue: None,
            avatar_color: None,
            initial_message: "Start".to_string(),
            provider: None,
            model: None,
            reasoning_effort: None,
            provider_account_id: None,
            operation_id: None,
        })
        .unwrap();
    let tid = agents.ensure_thread_id(&agent.id).unwrap();
    let threads = db.thread_store();
    for step in 1..=3 {
        let m = ConversationMessage::new_simple(
            "m-stream",
            ConversationMessageAuthor::Assistant,
            "x".repeat(step * 256),
            "2026-11-26T00:01:00.000Z",
            if step < 3 {
                ConversationMessageStatus::Streaming
            } else {
                ConversationMessageStatus::Completed
            },
        );
        let snap = ConversationSnapshot {
            agent_id: agent.id.clone(),
            thread_id: Some(tid.clone()),
            active_turn_id: None,
            revision: step as u64,
            messages: vec![m],
        };
        threads
            .persist_conversation(&snap, "conversation.updated", &serde_json::json!({}), None)
            .unwrap();
    }
    let batches = db.compact_thread_events().unwrap();
    assert!(!batches.is_empty());
    assert!(batches.last().unwrap().completed_thread);
    assert!(db.compact_thread_events().unwrap().is_empty());
}

#[test]
fn smoke_3_agents_threads_mailbox_fanout_barrier_and_team_chat() {
    let tmp = TempDir::new().expect("tempdir");
    let db = OpenBotDatabase::open(tmp.path().join("data").join("openbot.db")).unwrap();
    let agents = db.agent_store_at(tmp.path().join("OpenBot")).unwrap();

    let a1 = agents
        .create_agent(&CreateAgentInput {
            name: "Alpha".to_string(),
            description: "Lead".to_string(),
            avatar_seed: "alpha".to_string(),
            avatar_hue: Some(AvatarHue::Blue),
            avatar_color: None,
            initial_message: "Init Alpha".to_string(),
            provider: Some(AgentProviderId::Codex),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some(AgentReasoningEffort::High),
            provider_account_id: None,
            operation_id: None,
        })
        .unwrap();
    let mut upd = UpdateAgentInput {
        agent_id: a1.id.clone(),
        ..Default::default()
    };
    upd.title = Some("Principal Engineer".to_string());
    let updated = agents.update_agent(&upd, None).unwrap();
    assert_eq!(updated.title, "Principal Engineer");

    let tid = agents.ensure_thread_id(&a1.id).unwrap();
    let threads = db.thread_store();
    let m0 = ConversationMessage::new_simple(
        "msg-0",
        ConversationMessageAuthor::User,
        "Pin this please",
        "2026-11-26T00:10:00.000Z",
        ConversationMessageStatus::Completed,
    );
    let m1 = ConversationMessage::new_simple(
        "msg-1",
        ConversationMessageAuthor::Assistant,
        "Pinned response",
        "2026-11-26T00:10:01.000Z",
        ConversationMessageStatus::Completed,
    );
    threads
        .persist_conversation(
            &ConversationSnapshot {
                agent_id: a1.id.clone(),
                thread_id: Some(tid.clone()),
                active_turn_id: None,
                revision: 1,
                messages: vec![m0, m1],
            },
            "conversation.updated",
            &serde_json::json!({}),
            None,
        )
        .unwrap();
    let ordinals: Vec<i64> = db
        .with_conn(|conn| {
            let mut s = conn.prepare(
                "SELECT ordinal FROM projection_thread_messages WHERE thread_id = ?1 ORDER BY ordinal",
            )?;
            let rows = s.query_map([&tid], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap();
    assert_eq!(ordinals, vec![0, 1]);

    assert!(threads.pins().set_by_person(&a1.id, None, "msg-1", true, None).unwrap().is_ok());
    let refusal = threads.pins().set_by_agent(&a1.id, &tid, "msg-1", false).unwrap();
    assert_eq!(refusal, Err(PinRefusalReason::PinnedByPerson));
    assert_eq!(PinRefusalReason::PinnedByPerson.as_str(), "pinned-by-person");

    let waits = db.agent_wait_store();
    for idx in 0..INPUT_LIMITS.agent_waits {
        waits
            .open(OpenAgentWaitInput {
                agent_id: a1.id.clone(),
                request_message_id: format!("req-{idx}"),
                request_text: "Wait for webhook".to_string(),
                note: format!("wait-{idx}"),
                routine_id: None,
            })
            .unwrap();
    }
    assert!(matches!(
        waits.open(OpenAgentWaitInput {
            agent_id: a1.id.clone(),
            request_message_id: "req-overflow".to_string(),
            request_text: "Overflow".to_string(),
            note: "overflow".to_string(),
            routine_id: None,
        }),
        Err(StorageError::LimitExceeded(_))
    ));

    let mailbox = db.mailbox_store_at(tmp.path().join("OpenBot").join("Shared")).unwrap();
    let q1 = mailbox
        .enqueue(EnqueueInput {
            recipient_agent_ids: vec![a1.id.clone()],
            text: "First".to_string(),
            ..Default::default()
        })
        .unwrap();
    let q2 = mailbox
        .enqueue(EnqueueInput {
            recipient_agent_ids: vec![a1.id.clone()],
            text: "Second".to_string(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(q1.deliveries[0].position, Some(1));
    assert_eq!(q2.deliveries[0].position, Some(2));

    let fanout = mailbox
        .send_message(SendAgentMessageInput {
            sender_agent_id: "orch-bot".to_string(),
            recipient_agent_ids: vec!["worker-1".to_string(), "worker-2".to_string()],
            text: "Audit subsystem".to_string(),
            reply_to_message_id: None,
            expects_reply: Some(true),
            sender_task_id: None,
            requester_ids: Vec::new(),
            source_paths: Vec::new(),
            idempotency_key: Some("fanout-key".to_string()),
        })
        .unwrap();
    let d_w1 = fanout.deliveries[0].id.clone();
    let d_w2 = fanout.deliveries[1].id.clone();
    mailbox.mark_starting_at(&d_w1, 1_000).unwrap();
    mailbox.mark_running(&d_w1, "turn-w1").unwrap();
    mailbox.mark_starting_at(&d_w2, 1_000).unwrap();
    mailbox.mark_running(&d_w2, "turn-w2").unwrap();

    let reply_w1 = format_teammate_reply(&ParsedTeammateReply {
        status: TeammateReplyStatus::Done,
        result: "Worker 1 complete".to_string(),
        evidence: "src/a.rs:1".to_string(),
        unblock: None,
    });
    let ans_w1 = mailbox
        .send_message(SendAgentMessageInput {
            sender_agent_id: "worker-1".to_string(),
            recipient_agent_ids: vec!["orch-bot".to_string()],
            text: reply_w1,
            reply_to_message_id: Some(fanout.message_id.clone()),
            expects_reply: Some(false),
            sender_task_id: None,
            requester_ids: Vec::new(),
            source_paths: Vec::new(),
            idempotency_key: None,
        })
        .unwrap();
    mailbox
        .mark_terminal(&d_w1, QueueDeliveryStatus::Completed, None)
        .unwrap();
    assert!(
        mailbox.next_queued_at("orch-bot", None, 2_000).is_none(),
        "partial fan-out reply must be held until sibling worker finishes"
    );

    let reply_w2 = format_teammate_reply(&ParsedTeammateReply {
        status: TeammateReplyStatus::Done,
        result: "Worker 2 complete".to_string(),
        evidence: "src/b.rs:1".to_string(),
        unblock: None,
    });
    let ans_w2 = mailbox
        .send_message(SendAgentMessageInput {
            sender_agent_id: "worker-2".to_string(),
            recipient_agent_ids: vec!["orch-bot".to_string()],
            text: reply_w2,
            reply_to_message_id: Some(fanout.message_id.clone()),
            expects_reply: Some(false),
            sender_task_id: None,
            requester_ids: Vec::new(),
            source_paths: Vec::new(),
            idempotency_key: None,
        })
        .unwrap();
    mailbox
        .mark_terminal(&d_w2, QueueDeliveryStatus::Completed, None)
        .unwrap();
    let released_head = mailbox.next_queued_at("orch-bot", None, 3_000).unwrap();
    assert_eq!(released_head.delivery.id, ans_w1.deliveries[0].id);
    let siblings = mailbox.replies_to_start_with_at(&released_head.delivery.id, 3_000);
    assert_eq!(siblings.len(), 1);
    assert_eq!(siblings[0].delivery.id, ans_w2.deliveries[0].id);

    let dms = db.team_chat_store();
    let dm_id = direct_thread_id("user-z", "user-a");
    assert_eq!(dm_id, direct_thread_id("user-a", "user-z"));
    assert_eq!(dm_id.len(), 35);
    assert!(dm_id.starts_with("dm-"));
    dms.send_message(&SendDirectMessageInput {
        client_message_id: "dm-1".to_string(),
        sender_member_id: "user-a".to_string(),
        recipient_member_id: "user-z".to_string(),
        text: "Direct ping".to_string(),
        created_at: None,
    })
    .unwrap();
    assert_eq!(dms.read_conversation("user-z", "user-a").unwrap().messages.len(), 1);

    for idx in 1..INPUT_LIMITS.agents {
        agents
            .get_or_create(&format!("cap-bot-{idx}"), Some(&format!("Bot {idx}")), None)
            .unwrap();
    }
    assert!(matches!(
        agents.get_or_create("cap-overflow", Some("Overflow"), None),
        Err(StorageError::LimitExceeded(_))
    ));
    agents.hard_delete_agent(&a1.id).unwrap();
    assert!(agents.get(&a1.id).is_none());
}

#[test]
fn smoke_4_channels_rebuild_memories_fts5_cjk_site_and_history_redaction() {
    let tmp = TempDir::new().expect("tempdir");
    let db = OpenBotDatabase::open(tmp.path().join("data").join("openbot.db")).unwrap();
    let channels = db.channel_store();

    let draft = ChannelDraft {
        name: "core-eng".to_string(),
        title: "Core Engineering".to_string(),
        instructions: "Coordinate backend releases.".to_string(),
        members: vec![
            ChannelMember {
                agent_id: "bot-lead".to_string(),
            },
            ChannelMember {
                agent_id: "bot-qa".to_string(),
            },
        ],
        lead_agent_id: Some("bot-lead".to_string()),
        isolated: Some(false),
        browser: Some(ChannelBrowser::Members),
        icon: None,
    };
    let initial = channels.create("ch-eng", draft);
    let ch_msg1 = ChannelMessage {
        id: "cm-1".to_string(),
        channel_id: "ch-eng".to_string(),
        sequence: 1,
        author: ChannelMessageAuthor {
            kind: ChannelMessageAuthorKind::Member,
            id: "local".to_string(),
            name: "Owner".to_string(),
        },
        task_id: None,
        superseded: false,
        message: ConversationMessage::new_simple(
            "cm-1",
            ConversationMessageAuthor::User,
            "Deploy release v1",
            "2026-11-26T01:00:00.000Z",
            ConversationMessageStatus::Completed,
        ),
        deleted: None,
        sender_task_id: None,
    };
    let ch_msg2 = ChannelMessage {
        id: "cm-2".to_string(),
        channel_id: "ch-eng".to_string(),
        sequence: 2,
        author: ChannelMessageAuthor {
            kind: ChannelMessageAuthorKind::Agent,
            id: "bot-lead".to_string(),
            name: "Lead".to_string(),
        },
        task_id: None,
        superseded: false,
        message: ConversationMessage::new_simple(
            "cm-2",
            ConversationMessageAuthor::Assistant,
            "Deploying now",
            "2026-11-26T01:00:02.000Z",
            ConversationMessageStatus::Completed,
        ),
        deleted: None,
        sender_task_id: None,
    };
    channels
        .commit(
            "op-ch-create",
            ChannelChange {
                channel: initial,
                messages: vec![ch_msg1, ch_msg2],
                tasks: Vec::new(),
                assignments: Vec::new(),
            },
        )
        .unwrap();

    let stored_msgs = channels.messages("ch-eng", None, None).unwrap();
    assert_eq!(stored_msgs.iter().map(|m| m.sequence).collect::<Vec<_>>(), vec![1, 2]);
    let ctx_lead = channels.context("ch-eng", "bot-lead").unwrap();
    let ctx_qa = channels.context("ch-eng", "bot-qa").unwrap();
    assert_ne!(ctx_lead.thread_id, ctx_qa.thread_id);
    let dup_thread_err = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO projection_channel_contexts (channel_id, agent_id, thread_id, through_sequence, summary_version)
             VALUES ('ch-eng', 'bot-extra', ?1, 0, 0)",
            [&ctx_lead.thread_id],
        )
        .map_err(StorageError::from)
    });
    assert!(dup_thread_err.is_err(), "projection_channel_contexts UNIQUE(thread_id) must reject reuse");

    let ch_mem_store = db.channel_memory_store();
    let ch_mem = ch_mem_store
        .create_manual("ch-eng", "Release branch is feat/openbot-fusion")
        .unwrap();
    db.with_conn(|conn| {
        conn.execute("DELETE FROM projection_channel_messages WHERE channel_id = 'ch-eng'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(channels.messages("ch-eng", None, None).unwrap().is_empty());
    channels.rebuild("ch-eng").unwrap();
    assert_eq!(channels.messages("ch-eng", None, None).unwrap().len(), 2);
    assert_eq!(ch_mem_store.list("ch-eng").unwrap().len(), 1);
    assert_eq!(ch_mem_store.list("ch-eng").unwrap()[0].entry.id, ch_mem.entry.id);

    let mems = db.agent_memory_store();
    assert_eq!(mems.budget_bytes(), AGENT_MEMORY_CONTEXT_BUDGET_BYTES);
    assert_eq!(AGENT_MEMORY_CONTEXT_BUDGET_BYTES, 4_096);
    let m_en = mems
        .create_manual("bot-lead", "Production Kubernetes deployment uses blue-green rollout")
        .unwrap();
    let m_cjk = mems
        .create_manual("bot-lead", "生产环境数据库迁移必须在凌晨执行")
        .unwrap();
    let _m_site = mems
        .save_automatic(SaveAutomaticMemoryInput {
            agent_id: "bot-lead".to_string(),
            text: "Use SSO login button on dashboard".to_string(),
            memory_id: None,
            source_turn_id: "t-site".to_string(),
            site: Some("example.com".to_string()),
            expected_selection_revision: None,
            expected_updated_at: None,
        })
        .unwrap()
        .unwrap();

    let ctx = build_memory_context(&mems.list("bot-lead").unwrap(), None, None, None);
    let formatted = format_memory_context(&ctx);
    assert!(formatted.contains("<openbot_saved_memories>"));
    assert!(formatted.len() <= 8_192);

    let prefix_hits = mems.search("bot-lead", "deployments", 5).unwrap();
    assert!(prefix_hits.memories.iter().any(|r| r.memory.entry.id == m_en.entry.id));
    let cjk_hits = mems.search("bot-lead", "数据库迁移", 5).unwrap();
    assert!(cjk_hits.memories.iter().any(|r| r.memory.entry.id == m_cjk.entry.id));
    let host_hits = mems.for_host("bot-lead", "app.sub.example.com", 5).unwrap();
    assert_eq!(host_hits.len(), 1);

    let agents = db.agent_store_at(tmp.path().join("OpenBot")).unwrap();
    let hist_agent = agents
        .get_or_create("bot-hist", Some("Historian"), None)
        .unwrap();
    let hist_tid = agents.ensure_thread_id(&hist_agent.id).unwrap();
    let secret_token = "sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz1234567890ABCD";
    db.thread_store()
        .persist_conversation(
            &ConversationSnapshot {
                agent_id: hist_agent.id.clone(),
                thread_id: Some(hist_tid.clone()),
                active_turn_id: None,
                revision: 1,
                messages: vec![
                    ConversationMessage::new_simple(
                        "hm-1",
                        ConversationMessageAuthor::User,
                        format!("Deploy key is {secret_token} for staging"),
                        "2026-11-26T01:05:00.000Z",
                        ConversationMessageStatus::Completed,
                    ),
                    ConversationMessage::new_simple(
                        "hm-2",
                        ConversationMessageAuthor::Assistant,
                        format!("Stored staging token {secret_token} safely"),
                        "2026-11-26T01:05:01.000Z",
                        ConversationMessageStatus::Completed,
                    ),
                ],
            },
            "conversation.updated",
            &serde_json::json!({}),
            None,
        )
        .unwrap();

    let history = db.history_tools();
    let search_out = history
        .history_search(
            &hist_agent.id,
            &hist_tid,
            Some(&serde_json::json!({ "query": "staging" })),
        )
        .unwrap();
    let search_json = serde_json::to_string(&search_out).unwrap();
    assert!(!search_json.contains(secret_token));
    assert!(search_json.contains("[redacted]"));

    let read_out = history
        .history_read(&hist_agent.id, &hist_tid, Some(&serde_json::json!({})))
        .unwrap();
    let read_json = serde_json::to_string(&read_out).unwrap();
    assert!(!read_json.contains(secret_token));
    assert!(read_json.contains("[redacted]"));
}

#[test]
fn smoke_5_workflows_routines_crm_browser_mcp_json_and_shared_agent_data() {
    let tmp = TempDir::new().expect("tempdir");
    let db = OpenBotDatabase::open(tmp.path().join("data").join("openbot.db")).unwrap();

    // 1. WorkflowStore + WorkflowRunStore
    let wf_store = db.workflow_store();
    let mut trigger_fields = BTreeMap::new();
    trigger_fields.insert(
        "triggerType".to_string(),
        WorkflowFieldValue::Text("manual".to_string()),
    );
    let graph = WorkflowGraph {
        nodes: vec![WorkflowNode {
            id: "node-trig".to_string(),
            kind: WorkflowNodeKind::Trigger,
            at: WorkflowPoint { x: 100.0, y: 100.0 },
            size: None,
            locked: None,
            fields: trigger_fields,
            schedule: None,
        }],
        edges: Vec::new(),
        strokes: Vec::new(),
    };
    let wf = wf_store
        .create(
            CreateWorkflowInput {
                title: "Release Pipeline".to_string(),
                graph: Some(graph.clone()),
                paused: Some(false),
            },
            Some(1_700_000_000_000),
            WorkflowValuesMode::New,
        )
        .unwrap();
    let saved_rev = match wf_store
        .save_graph(&wf.summary.id, wf.revision, &graph, Some(1_700_000_010_000))
        .unwrap()
    {
        WorkflowSaveResult::Saved { revision } => revision,
        other => panic!("expected Saved, got {other:?}"),
    };
    assert!(matches!(
        wf_store
            .save_graph(&wf.summary.id, wf.revision, &graph, Some(1_700_000_020_000))
            .unwrap(),
        WorkflowSaveResult::Conflict { .. }
    ));

    let run_store = db.workflow_run_store();
    let run_input = NewWorkflowRun {
        workflow_id: wf.summary.id.clone(),
        trigger_node_id: Some("node-trig".to_string()),
        from_node_id: None,
        source: WorkflowRunSource::Manual,
        scheduled_for: None,
        idempotency_key: "idem-wf-1".to_string(),
        revision: saved_rev,
        graph: graph.clone(),
        payload: None,
    };
    let (run1, created1) = run_store.start_run(run_input.clone(), Some(1_700_000_030_000)).unwrap();
    let (run2, created2) = run_store.start_run(run_input, Some(1_700_000_031_000)).unwrap();
    assert!(created1);
    assert!(!created2);
    assert_eq!(run1.id, run2.id);
    assert_eq!(run1.number, 1);

    // 2. AgentRoutineStore (>= 180s minimum interval)
    let routines = db.agent_routine_store();
    let routine = routines
        .create(
            "agent-r1",
            &RoutineInputFields {
                name: "Health Check".to_string(),
                instruction: "Verify endpoints".to_string(),
                active: true,
                timezone: "UTC".to_string(),
                schedule: RoutineSchedule::Interval {
                    amount: 5,
                    unit: RoutineIntervalUnit::Minutes,
                    anchor_at: "2026-11-26T00:00:00.000Z".to_string(),
                },
                limit_policy: None,
                missed_policy: None,
                run_in: None,
            },
            Some(1_700_000_000_000),
        )
        .unwrap();
    assert_eq!(routines.list("agent-r1").unwrap().len(), 1);
    assert_eq!(routines.get("agent-r1", &routine.id).unwrap().unwrap().name, "Health Check");
    assert!(
        routines
            .create(
                "agent-r1",
                &RoutineInputFields {
                    name: "Too Fast".to_string(),
                    instruction: "Invalid <3m".to_string(),
                    active: true,
                    timezone: "UTC".to_string(),
                    schedule: RoutineSchedule::Interval {
                        amount: 2,
                        unit: RoutineIntervalUnit::Minutes,
                        anchor_at: "2026-11-26T00:00:00.000Z".to_string(),
                    },
                    limit_policy: None,
                    missed_policy: None,
                    run_in: None,
                },
                Some(1_700_000_000_000),
            )
            .is_err()
    );

    // 3. CrmStore + CrmAgentView (base_revision, private fields masking & search-oracle exclusion, propose -> decide)
    let crm = db.crm_store();
    let owner_actor = CrmActor {
        kind: CrmActorKind::Person,
        id: Some("owner-1".to_string()),
        name: Some("Alice".to_string()),
    };
    let cust_id = match crm
        .apply(
            &CrmChange::CustomerCreate {
                customer: CrmCustomerInput {
                    name: "Acme Secret Corp".to_string(),
                    currency: "USD".to_string(),
                    billing_email: Some("vip-secret@acme.example".to_string()),
                    domain: Some("acme-public.example".to_string()),
                    notes: Some("Confidential acquisition target".to_string()),
                    ..Default::default()
                },
            },
            &owner_actor,
        )
        .unwrap()
    {
        CrmApplyResult::Applied { id: Some(id) } => id,
        other => panic!("expected Applied with customer id, got {other:?}"),
    };
    let cust = crm.customer(&cust_id).unwrap();
    assert!(matches!(
        crm.apply(
            &CrmChange::CustomerUpdate {
                customer_id: cust_id.clone(),
                patch: CrmCustomerPatch {
                    phone: Some("+1-555-0100".to_string()),
                    ..Default::default()
                },
                base_revision: cust.revision + 99,
            },
            &owner_actor,
        )
        .unwrap(),
        CrmApplyResult::Conflict { .. }
    ));

    crm.apply(
        &CrmChange::AgentAccess {
            agent_ids: vec!["crm-bot".to_string()],
        },
        &owner_actor,
    )
    .unwrap();
    crm.apply(
        &CrmChange::PrivateFields {
            fields: vec![
                CrmPrivateField::Name,
                CrmPrivateField::BillingEmail,
                CrmPrivateField::Notes,
            ],
        },
        &owner_actor,
    )
    .unwrap();

    let view = db.crm_agent_view();
    let masked = view.customer("crm-bot", &cust_id).unwrap();
    assert_eq!(masked.get("name").and_then(|v| v.as_str()), Some("Customer 1"));
    assert_eq!(masked.get("billingEmail").and_then(|v| v.as_str()), Some("hidden"));
    assert_eq!(masked.get("notes").and_then(|v| v.as_str()), Some("hidden"));

    let oracle_leak = view
        .find(
            "crm-bot",
            &CrmAgentFindInput {
                query: Some("Confidential".to_string()),
                status: None,
                tag: None,
                payment: None,
                cursor: None,
                limit: None,
            },
        )
        .unwrap();
    assert_eq!(
        oracle_leak.get("customers").and_then(|v| v.as_array()).unwrap().len(),
        0,
        "private field token must never match via search oracle"
    );
    let visible_hit = view
        .find(
            "crm-bot",
            &CrmAgentFindInput {
                query: Some("acme-public.example".to_string()),
                status: None,
                tag: None,
                payment: None,
                cursor: None,
                limit: None,
            },
        )
        .unwrap();
    assert_eq!(visible_hit.get("customers").and_then(|v| v.as_array()).unwrap().len(), 1);

    let prop_id = view
        .propose(
            "crm-bot",
            &CrmAgentProposeInput {
                change: CrmAgentChange::CustomerUpdate {
                    customer_id: cust_id.clone(),
                    patch: CrmAgentCustomerFields {
                        phone: Some("+1-555-0199".to_string()),
                        ..Default::default()
                    },
                },
                reason: "Verified support phone".to_string(),
                thread_id: None,
            },
        )
        .unwrap();
    crm.apply(
        &CrmChange::ProposalDecide {
            proposal_id: prop_id,
            approve: true,
        },
        &owner_actor,
    )
    .unwrap();
    assert_eq!(crm.customer(&cust_id).unwrap().phone, "+1-555-0199");
}

#[test]
fn smoke_6_browser_mcp_json_preferences_and_shared_agent_data_authorizer() {
    let tmp = TempDir::new().expect("tempdir");
    let db = OpenBotDatabase::open(tmp.path().join("data").join("openbot.db")).unwrap();

    // 1. BrowserProfileStore + McpServerStore
    let profiles = db.browser_profile_store();
    let prof = profiles.create("Research Profile", AvatarHue::Green).unwrap();
    assert!(is_browser_profile_id(&prof.id));
    assert_eq!(profiles.list().unwrap().len(), 1);

    let mcp = db.mcp_server_store();
    let saved_mcp = mcp
        .save(
            &McpServerConfig {
                id: String::new(),
                name: "docs-mcp".to_string(),
                transport: McpTransport::Http,
                enabled: true,
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
                env_passthrough: Vec::new(),
                working_directory: String::new(),
                url: "https://mcp.example.com/sse".to_string(),
                headers: vec![McpKeyValue {
                    key: "Authorization".to_string(),
                    value: "Bearer tok".to_string(),
                }],
            },
            None,
        )
        .unwrap();
    assert_eq!(mcp.list().unwrap().len(), 1);
    assert_eq!(mcp.get(&saved_mcp.id).unwrap().unwrap().name, "docs-mcp");
    assert!(
        mcp.save(
            &McpServerConfig {
                id: String::new(),
                name: "openbot".to_string(),
                transport: McpTransport::Stdio,
                enabled: true,
                command: "npx".to_string(),
                args: Vec::new(),
                env: Vec::new(),
                env_passthrough: Vec::new(),
                working_directory: String::new(),
                url: String::new(),
                headers: Vec::new(),
            },
            None,
        )
        .is_err()
    );

    // 2. JsonPreferenceStore (0600) + VersionedJsonStore forward-version .v<current> fallback
    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct BusyPref {
        mode: String,
    }
    let pref_path = resolve_preference_file(tmp.path(), preference_files::BUSY_MESSAGE_MODE_V1);
    let pref_store = JsonPreferenceStore::new(
        &pref_path,
        BusyPref {
            mode: "steer".to_string(),
        },
    );
    pref_store
        .set(BusyPref {
            mode: "queue".to_string(),
        })
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&pref_path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let reloaded_pref = JsonPreferenceStore::new(
        &pref_path,
        BusyPref {
            mode: "steer".to_string(),
        },
    );
    assert_eq!(
        reloaded_pref.load().unwrap(),
        BusyPref {
            mode: "queue".to_string()
        }
    );

    let ver_path = resolve_preference_file(tmp.path(), preference_files::DYNAMIC_ISLAND_PREFERENCE_V1);
    fs::write(&ver_path, br#"{"version": 99, "futureFlag": true}"#).unwrap();
    let vstore = VersionedJsonStore::new(&ver_path, 3, serde_json::json!({"enabled": true}));
    let load_res = vstore.load().unwrap();
    assert_eq!(load_res.state, VersionedJsonFileState::Kept);
    assert_eq!(load_res.write_path, versioned_json_fallback_path(&ver_path, 3));
    vstore.set(serde_json::json!({"enabled": false})).unwrap();
    assert!(versioned_json_fallback_path(&ver_path, 3).exists());
    let main_still_future: serde_json::Value =
        serde_json::from_slice(&fs::read(&ver_path).unwrap()).unwrap();
    assert_eq!(main_still_future["version"], 99);

    // 3. SharedAgentDataDb authorizer & <= 64 table cap
    let shared = SharedAgentDataDb::open(
        tmp.path().join("OpenBot").join("Shared").join("Data").join("agent-data.db"),
    )
    .unwrap();
    shared
        .execute(
            "agent-alpha",
            "CREATE TABLE metrics (id INTEGER PRIMARY KEY, label TEXT NOT NULL)",
            &[],
        )
        .unwrap();
    assert_eq!(shared.table_owner("metrics").unwrap().as_deref(), Some("agent-alpha"));

    // Cross-agent DML (INSERT + SELECT) is permitted on shared user tables
    shared
        .execute(
            "agent-beta",
            "INSERT INTO metrics (label) VALUES (?1)",
            &[serde_json::json!("latency-p99")],
        )
        .unwrap();
    let q = shared
        .query("agent-beta", "SELECT label FROM metrics WHERE id = 1", &[])
        .unwrap();
    assert_eq!(q.rows.len(), 1);
    assert_eq!(q.rows[0][0], AgentCellValue::Text("latency-p99".to_string()));

    // Non-owner DDL (DROP TABLE / ALTER TABLE / remove_table) is denied
    assert!(shared.execute("agent-beta", "DROP TABLE metrics", &[]).is_err());
    assert!(
        shared
            .execute("agent-beta", "ALTER TABLE metrics ADD COLUMN extra TEXT", &[])
            .is_err()
    );
    assert!(matches!(
        shared.remove_table("agent-beta", "metrics"),
        Err(StorageError::Forbidden(_))
    ));

    // ATTACH, mutating PRAGMA, and metadata table access are denied
    assert!(
        shared
            .execute("agent-alpha", "ATTACH DATABASE ':memory:' AS aux", &[])
            .is_err()
    );
    assert!(
        shared
            .execute("agent-alpha", "PRAGMA writable_schema = ON", &[])
            .is_err()
    );
    assert!(
        shared
            .query("agent-alpha", "SELECT * FROM _openbot_tables", &[])
            .is_err()
    );
    assert!(
        shared
            .execute("agent-alpha", "DELETE FROM _openbot_tables", &[])
            .is_err()
    );

    // Enforce <= 64 shared tables cap
    for idx in 1..64 {
        shared
            .execute(
                "agent-alpha",
                &format!("CREATE TABLE extra_tbl_{idx} (id INTEGER PRIMARY KEY)"),
                &[],
            )
            .unwrap();
    }
    assert_eq!(shared.list_tables().unwrap().len(), 64);
    assert!(matches!(
        shared.execute(
            "agent-alpha",
            "CREATE TABLE extra_tbl_overflow (id INTEGER PRIMARY KEY)",
            &[],
        ),
        Err(StorageError::LimitExceeded(_))
    ));
    shared.quick_check().unwrap();
}
