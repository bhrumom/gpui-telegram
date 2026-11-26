use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use orca_core::OrcaIdeErrorCode;
use orca_core::orchestration_protocol::{
    DispatchStatus, ExecutionHostRecord, GateStatus, HostSelectionCriteria, MailboxMessageType,
    MailboxPriority, MailboxRoute, MailboxTarget, MergeGate, OrchestrationTaskStatus, ReviewGate,
    WorkerLivenessState,
};

use super::*;

#[test]
fn engine_is_send_sync_and_initializes_all_v51_tables() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OrchestrationEngine>();

    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("orchestration.sqlite");
    let engine = OrchestrationEngine::open(&db_path).expect("open wal db");

    let conn = engine.conn.lock();
    let expected_tables = [
        "runs",
        "coordinator_runs",
        "tasks",
        "dispatch_contexts",
        "worker_dispatches",
        "worker_terminal_resources",
        "worker_terminal_archives",
        "messages",
        "deliveries",
        "questions",
        "decision_gates",
        "mutation_receipts",
        "attempt_observation_facts",
        "federated_dispatches",
        "remote_dispatch_attachments",
        "federation_relay_items",
        "remote_questions",
        "peer_mail_conversations",
        "peer_mail_messages",
        "peer_runs",
    ];
    for tbl in expected_tables {
        let found: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                rusqlite::params![tbl],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(found.as_deref(), Some(tbl), "missing table {tbl}");
    }
}

#[test]
fn dag_readiness_and_cycle_rejection_and_generation_fencing() {
    let engine = OrchestrationEngine::in_memory().expect("in_memory");
    let run = engine
        .create_run(CreateRunRequest {
            run_id: Some("run-dag".into()),
            title: "DAG Run".into(),
            coordinator_orca_session_id: None,
            mailbox_is_local_only: true,
            now_ms: Some(1_000),
        })
        .expect("create_run");

    let t1 = engine
        .create_task(CreateTaskRequest {
            run_id: run.run_id.clone(),
            task_id: Some("t1".into()),
            title: "Build".into(),
            depends_on: vec![],
            parent_id: None,
            nesting_depth: None,
            now_ms: Some(1_001),
        })
        .expect("t1");
    assert_eq!(t1.status, OrchestrationTaskStatus::Ready);
    assert_eq!(t1.nesting_depth, 1);

    let t2 = engine
        .create_task(CreateTaskRequest {
            run_id: run.run_id.clone(),
            task_id: Some("t2".into()),
            title: "Test".into(),
            depends_on: vec!["t1".into()],
            parent_id: None,
            nesting_depth: Some(2),
            now_ms: Some(1_002),
        })
        .expect("t2");
    assert_eq!(t2.status, OrchestrationTaskStatus::Pending);

    // Self-cycle or unknown dependency must be rejected with UnknownTask.
    let self_cycle = engine.create_task(CreateTaskRequest {
        run_id: run.run_id.clone(),
        task_id: Some("t3".into()),
        title: "Cyclic".into(),
        depends_on: vec!["t3".into()],
        parent_id: None,
        nesting_depth: None,
        now_ms: Some(1_003),
    });
    assert_eq!(
        self_cycle.expect_err("self-cycle rejected").ide_error_code(),
        Some(OrcaIdeErrorCode::UnknownTask)
    );

    let ready = engine.ready_tasks(&run.run_id).expect("ready");
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].task_id, "t1");

    // Start worker on t1 and verify generation fencing.
    let started = engine
        .start_worker(StartWorkerRequest {
            run_id: run.run_id.clone(),
            task_id: "t1".into(),
            dispatch_id: Some("d1".into()),
            host_id: None,
            agent_id: "claude".into(),
            worktree_id: Some("wt-1".into()),
            depth: None,
            capability_manifest: None,
            launch_token: None,
            follow_up_of_dispatch_id: None,
            keep_open: false,
            tab_id: Some("tab-1".into()),
            pty_incarnation_id: Some("pty-1".into()),
            now_ms: Some(1_010),
        })
        .expect("start_worker");
    assert_eq!(started.dispatch.generation, 1);

    let stale = engine.report_liveness("d1", 99, WorkerLivenessState::Alive, Some(1_011));
    assert_eq!(
        stale.expect_err("stale gen rejected").ide_error_code(),
        Some(OrcaIdeErrorCode::StaleDispatch)
    );
}

#[test]
fn unverifiable_liveness_never_confirms_exit_or_retries_and_wakes_coordinator() {
    let engine = OrchestrationEngine::in_memory().expect("in_memory");
    let mut wakes = engine.subscribe_wakes();
    let run = engine
        .create_run(CreateRunRequest {
            run_id: Some("run-live".into()),
            title: "Liveness".into(),
            coordinator_orca_session_id: None,
            mailbox_is_local_only: true,
            now_ms: Some(2_000),
        })
        .expect("run");
    engine
        .create_task(CreateTaskRequest {
            run_id: run.run_id.clone(),
            task_id: Some("t1".into()),
            title: "Run job".into(),
            depends_on: vec![],
            parent_id: None,
            nesting_depth: None,
            now_ms: Some(2_001),
        })
        .expect("task");
    engine
        .start_worker(StartWorkerRequest {
            run_id: run.run_id.clone(),
            task_id: "t1".into(),
            dispatch_id: Some("d-live".into()),
            host_id: None,
            agent_id: "codex".into(),
            worktree_id: None,
            depth: None,
            capability_manifest: None,
            launch_token: None,
            follow_up_of_dispatch_id: None,
            keep_open: false,
            tab_id: None,
            pty_incarnation_id: None,
            now_ms: Some(2_002),
        })
        .expect("start");

    let outcome = engine
        .report_liveness("d-live", 1, WorkerLivenessState::Unverifiable, Some(2_050))
        .expect("report_liveness");
    assert!(!outcome.confirmed_exit);
    assert!(!outcome.retry_authorized);
    assert!(outcome.coordinator_woken);
    assert_eq!(outcome.status, DispatchStatus::Dispatched);

    let mut saw_unverifiable_wake = false;
    while let Ok(ev) = wakes.try_recv() {
        if matches!(
            ev,
            OrchestrationWakeEvent::WorkerLivenessUnverifiable { ref dispatch_id, .. }
            if dispatch_id == "d-live"
        ) {
            saw_unverifiable_wake = true;
        }
    }
    assert!(saw_unverifiable_wake);
}

#[test]
fn auto_teardown_vs_keep_open_vs_failed_setup_and_follow_up_reopen() {
    let engine = OrchestrationEngine::in_memory().expect("in_memory");
    let run = engine
        .create_run(CreateRunRequest {
            run_id: Some("run-teardown".into()),
            title: "Teardown".into(),
            coordinator_orca_session_id: None,
            mailbox_is_local_only: true,
            now_ms: Some(3_000),
        })
        .expect("run");

    for id in ["t-auto", "t-keep", "t-fail-setup"] {
        engine
            .create_task(CreateTaskRequest {
                run_id: run.run_id.clone(),
                task_id: Some(id.into()),
                title: id.into(),
                depends_on: vec![],
                parent_id: None,
                nesting_depth: None,
                now_ms: Some(3_001),
            })
            .expect("task");
    }

    for (tid, did, keep_open) in [
        ("t-auto", "d-auto", false),
        ("t-keep", "d-keep", true),
        ("t-fail-setup", "d-fail", false),
    ] {
        engine
            .start_worker(StartWorkerRequest {
                run_id: run.run_id.clone(),
                task_id: tid.into(),
                dispatch_id: Some(did.into()),
                host_id: None,
                agent_id: "claude".into(),
                worktree_id: Some("wt-main".into()),
                depth: None,
                capability_manifest: None,
                launch_token: None,
                follow_up_of_dispatch_id: None,
                keep_open,
                tab_id: Some(format!("tab-{did}")),
                pty_incarnation_id: Some(format!("pty-{did}")),
                now_ms: Some(3_010),
            })
            .expect("start_worker");
    }

    // 1. Auto-teardown archives transcript and releases terminal resource.
    let auto_done = engine
        .complete_worker(CompleteWorkerRequest {
            dispatch_id: "d-auto".into(),
            expected_generation: 1,
            succeeded: true,
            exit_code: Some(0),
            transcript_text: Some("all tests passed\n".into()),
            keep_open: false,
            setup_failed: false,
            now_ms: Some(3_100),
        })
        .expect("auto done");
    assert_eq!(auto_done.resources_released, 1);
    assert!(!auto_done.kept_open);
    let archives = engine.read_worker_archive("d-auto").expect("archive");
    assert_eq!(archives.len(), 1);
    assert_eq!(archives[0].transcript_text, "all tests passed\n");

    // 2. `--keep-open` retains open resource until explicitly released.
    let keep_done = engine
        .complete_worker(CompleteWorkerRequest {
            dispatch_id: "d-keep".into(),
            expected_generation: 1,
            succeeded: true,
            exit_code: Some(0),
            transcript_text: Some("kept open".into()),
            keep_open: false,
            setup_failed: false,
            now_ms: Some(3_101),
        })
        .expect("keep done");
    assert_eq!(keep_done.resources_released, 0);
    assert!(keep_done.kept_open);
    assert!(
        engine
            .list_worker_resources("d-keep")
            .expect("res")[0]
            .released_at_ms
            .is_none()
    );
    assert_eq!(engine.release_worker_resources("d-keep", Some(3_200)).expect("rel"), 1);

    // 3. Failed setup stays open even without `--keep-open`.
    let fail_done = engine
        .complete_worker(CompleteWorkerRequest {
            dispatch_id: "d-fail".into(),
            expected_generation: 1,
            succeeded: false,
            exit_code: Some(2),
            transcript_text: Some("setup script crashed".into()),
            keep_open: false,
            setup_failed: true,
            now_ms: Some(3_102),
        })
        .expect("fail done");
    assert_eq!(fail_done.resources_released, 0);
    assert!(fail_done.kept_open);

    // 4. Sending message to finished `dispatch:d-auto` seamlessly reopens a linked follow-up.
    let sent = engine
        .send_message(SendMessageRequest {
            run_id: run.run_id.clone(),
            from_id: "coordinator".into(),
            target: parse_mailbox_target("dispatch:d-auto"),
            message_type: MailboxMessageType::Handoff,
            priority: MailboxPriority::Urgent,
            route: MailboxRoute::LocalOnly,
            body: "Please also add edge-case coverage".into(),
            dispatch_generation: None,
            now_ms: Some(3_300),
        })
        .expect("send follow-up");
    let fu = sent.follow_up.expect("follow-up created");
    assert_eq!(fu.original_dispatch_id, "d-auto");
    assert_eq!(
        sent.recipients,
        vec![format!("dispatch:{}", fu.follow_up_dispatch_id)]
    );

    let mailbox = engine
        .check_mailbox(CheckMailboxRequest {
            recipient_id: format!("dispatch:{}", fu.follow_up_dispatch_id),
            dispatch_id: Some(fu.follow_up_dispatch_id.clone()),
            expected_generation: Some(1),
            now_ms: Some(3_301),
        })
        .expect("check follow-up mailbox");
    assert_eq!(mailbox.len(), 1);
    assert_eq!(mailbox[0].message.body, "Please also add edge-case coverage");
    engine
        .ack_delivery(&mailbox[0].delivery_id, Some(1), Some(3_302))
        .expect("ack");
}

#[test]
fn idempotent_receipts_and_federation_and_gates() {
    let engine = OrchestrationEngine::in_memory().expect("in_memory");
    let calls = AtomicUsize::new(0);
    let payload = serde_json::json!({ "title": "Idempotent Run", "k": 1 });

    let (run1, replayed1) = engine
        .execute_idempotent("caller-a", "req-1", &payload, Some(4_000), |eng| {
            calls.fetch_add(1, Ordering::SeqCst);
            eng.create_run(CreateRunRequest {
                run_id: Some("run-idem".into()),
                title: "Idempotent Run".into(),
                coordinator_orca_session_id: None,
                mailbox_is_local_only: true,
                now_ms: Some(4_000),
            })
        })
        .expect("first exec");
    assert!(!replayed1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let (run2, replayed2): (OrchestrationRunRecord, bool) = engine
        .execute_idempotent("caller-a", "req-1", &payload, Some(4_001), |_eng| {
            calls.fetch_add(1, Ordering::SeqCst);
            unreachable!("replayed receipt must not execute closure")
        })
        .expect("replayed exec");
    assert!(replayed2);
    assert_eq!(run1.run_id, run2.run_id);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Payload mismatch with same key MUST return Conflict.
    let diff_payload = serde_json::json!({ "title": "Different", "k": 2 });
    let conflict = engine.execute_idempotent::<_, OrchestrationRunRecord, _>(
        "caller-a",
        "req-1",
        &diff_payload,
        Some(4_002),
        |_| unreachable!(),
    );
    assert_eq!(conflict.expect_err("mismatch conflict").code_str(), "conflict");

    // Local-only mailbox rejects server relay with MailboxIsLocal.
    let local_err = engine.send_message(SendMessageRequest {
        run_id: run1.run_id.clone(),
        from_id: "coord".into(),
        target: MailboxTarget::GroupAll,
        message_type: MailboxMessageType::Status,
        priority: MailboxPriority::Normal,
        route: MailboxRoute::ServerRelay {
            server_host_id: "srv-east".into(),
        },
        body: "hi".into(),
        dispatch_generation: None,
        now_ms: Some(4_005),
    });
    assert_eq!(
        local_err.expect_err("local only").ide_error_code(),
        Some(OrcaIdeErrorCode::MailboxIsLocal)
    );

    // Register remote host + fail-closed `--on` selection + federation relay + peer mail.
    engine
        .register_host(
            ExecutionHostRecord {
                host_id: "srv-gpu".into(),
                display_name: "GPU Worker".into(),
                capabilities: BTreeSet::from(["gpu".to_string(), "docker".to_string()]),
                online: true,
                protocol_version: 3,
                active_workers: 0,
                max_workers: 4,
                last_heartbeat_ms: 4_000,
            },
            true,
        )
        .expect("register host");

    let missing_explicit = engine.select_host(
        &HostSelectionCriteria {
            preferred_host_id: Some("srv-offline".into()),
            required_capabilities: vec![],
            min_protocol_version: 2,
            max_heartbeat_age_ms: 60_000,
        },
        Some(4_010),
    );
    assert!(missing_explicit.is_err());

    let fdisp = engine
        .dispatch_federated(DispatchFederatedRequest {
            dispatch_id: Some("fdisp-1".into()),
            run_id: run1.run_id.clone(),
            task_id: "t-remote".into(),
            home_host_id: "local".into(),
            selection: HostSelectionCriteria {
                preferred_host_id: Some("srv-gpu".into()),
                required_capabilities: vec!["gpu".into()],
                min_protocol_version: 3,
                max_heartbeat_age_ms: 60_000,
            },
            depth: Some(1),
            home_auto_release_at_ms: Some(9_000),
            now_ms: Some(4_010),
        })
        .expect("dispatch_federated");
    assert_eq!(fdisp.worker_host_id, "srv-gpu");

    let relay = engine
        .enqueue_relay_item(
            "fdisp-1",
            FederationRelayDirection::ToWorker,
            serde_json::json!({"cmd": "run"}),
            Some(4_011),
        )
        .expect("enqueue relay");
    assert_eq!(
        engine
            .list_unacked_relay_items("fdisp-1", Some(FederationRelayDirection::ToWorker))
            .expect("unacked")
            .len(),
        1
    );
    engine.ack_relay_item(relay.seq, Some(4_012)).expect("ack relay");
    assert!(
        engine
            .list_unacked_relay_items("fdisp-1", None)
            .expect("empty")
            .is_empty()
    );

    let (conv, _msg) = engine
        .send_peer_mail(SendPeerMailRequest {
            conversation_id: None,
            local_host_id: "local".into(),
            remote_host_id: "srv-gpu".into(),
            subject: "Sync".into(),
            from_host_id: "local".into(),
            to_host_id: "srv-gpu".into(),
            body: "Ready for batch 2".into(),
            now_ms: Some(4_015),
        })
        .expect("peer mail");
    assert_eq!(
        engine
            .list_peer_mail(Some(&conv.conversation_id), None)
            .expect("list peer mail")
            .len(),
        1
    );

    // Review + Merge Gate evaluation & WaitTimeout.
    engine
        .create_task(CreateTaskRequest {
            run_id: run1.run_id.clone(),
            task_id: Some("t-gate".into()),
            title: "Ship".into(),
            depends_on: vec![],
            parent_id: None,
            nesting_depth: None,
            now_ms: Some(4_020),
        })
        .expect("task gate");
    let gate = engine
        .create_gate(CreateGateRequest {
            run_id: run1.run_id.clone(),
            task_id: "t-gate".into(),
            gate_id: Some("gate-merge".into()),
            payload: DecisionGatePayload::Merge(MergeGate {
                gate_id: "gate-merge".into(),
                task_id: "t-gate".into(),
                ci_passed: false,
                conflicts_resolved: true,
                review_gates: vec![ReviewGate {
                    gate_id: "rg-1".into(),
                    task_id: "t-gate".into(),
                    required_approvals: 1,
                    approvals: vec!["alice".into()],
                    rejections: vec![],
                    status: GateStatus::Approved,
                    deadline_ms: Some(5_000),
                }],
                require_human_approval: false,
                human_approved: false,
                status: GateStatus::Pending,
                deadline_ms: Some(5_000),
            }),
            deadline_ms: Some(5_000),
            now_ms: Some(4_021),
        })
        .expect("create_gate");
    assert_eq!(gate.status, GateStatus::Pending);

    // Past deadline returns WaitTimeout.
    let timeout_err = engine.resolve_gate("gate-merge", ResolveGateAction::EvaluateOnly, Some(5_001));
    assert_eq!(
        timeout_err.expect_err("wait timeout").ide_error_code(),
        Some(OrcaIdeErrorCode::WaitTimeout)
    );
}
