use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tempfile::TempDir;

use openbot_contracts::crm::{
    CRM_CHANGE_KINDS, CRM_LIMITS, CRM_PRIVATE_FIELDS, CRM_PROPOSED_KINDS, CrmApplyResult,
    CrmChange, CrmConflictReason, CrmCustomerInput, CrmCustomerPatch, CrmPaymentPatch,
    CrmPlanPatch, CrmPrivateField, CrmProposedChange, CrmSplitPart, is_crm_amount, is_crm_currency,
};
use openbot_contracts::routines::{
    EventFilter, EventRoutineTrigger, EventScalar, RoutineDaySelection, RoutineFlowCanvas,
    RoutineFlowRoutine, RoutineFlowRoutineInfo, RoutineIntervalUnit, RoutineRunKind,
    RoutineRunSource, RoutineSchedule, RoutineSubDayUnit, RoutineTimeSelection,
    WebhookSecurityError, WebhookSignatureInput, is_event_filter_pointer, is_no_update_answer,
    is_valid_timezone, parse_cron_expression, quiet_routine_run, sign_webhook_body,
    verify_standard_webhook_signature, verify_webhook_signature,
};
use openbot_contracts::workflows::{
    WORKFLOW_FILE_FORMAT, WORKFLOW_FILE_VERSION, WORKFLOW_FORMAT, WORKFLOW_MEDIA_MODELS,
    WORKFLOW_NODE_REGISTRY, WORKFLOW_PORTS, WorkflowBranch, WorkflowDetail, WorkflowEdge,
    WorkflowFieldValue, WorkflowFileAsset, WorkflowGraph, WorkflowGraphProblem, WorkflowNode,
    WorkflowNodeKind, WorkflowPoint, WorkflowPortType, WorkflowPortableFile, WorkflowStroke,
    WorkflowTriggerType, workflow_graph_problem, workflow_loop_edges, workflow_needs_admin,
};

fn node(id: &str, kind: WorkflowNodeKind, x: f64, y: f64) -> WorkflowNode {
    WorkflowNode {
        id: id.into(),
        kind,
        at: WorkflowPoint { x, y },
        size: None,
        locked: None,
        fields: BTreeMap::new(),
        schedule: None,
    }
}

#[test]
fn workflow_dag_ports_limits_scc_loops_and_portable_file() {
    assert_eq!(WORKFLOW_PORTS.len(), 12);
    assert_eq!(WORKFLOW_NODE_REGISTRY.len(), 12);
    assert_eq!(WORKFLOW_MEDIA_MODELS.len(), 16);

    // Valid condition -> agent (yes) / output (no) graph
    let g = WorkflowGraph {
        nodes: vec![
            node("t1", WorkflowNodeKind::Trigger, 0.0, 0.0),
            node("c1", WorkflowNodeKind::Condition, 100.0, 0.0),
            node("a1", WorkflowNodeKind::Agent, 200.0, -50.0),
            node("o1", WorkflowNodeKind::Output, 200.0, 50.0),
        ],
        edges: vec![
            WorkflowEdge { id: "e1".into(), from: "t1".into(), to: "c1".into(), port_type: WorkflowPortType::Text, branch: None },
            WorkflowEdge { id: "e2".into(), from: "c1".into(), to: "a1".into(), port_type: WorkflowPortType::Text, branch: Some(WorkflowBranch::Yes) },
            WorkflowEdge { id: "e3".into(), from: "c1".into(), to: "o1".into(), port_type: WorkflowPortType::Text, branch: Some(WorkflowBranch::No) },
        ],
        strokes: vec![],
    };
    assert!(g.validate().is_ok());

    // Condition missing branch -> ConditionBranch; Annotation edge -> Annotation
    let mut missing_branch = g.clone();
    missing_branch.edges[1].branch = None;
    assert_eq!(workflow_graph_problem(&missing_branch), Some(WorkflowGraphProblem::ConditionBranch));

    let ann_graph = WorkflowGraph {
        nodes: vec![
            node("n1", WorkflowNodeKind::Note, 0.0, 0.0),
            node("o1", WorkflowNodeKind::Output, 50.0, 0.0),
        ],
        edges: vec![WorkflowEdge {
            id: "e-ann".into(),
            from: "n1".into(),
            to: "o1".into(),
            port_type: WorkflowPortType::Text,
            branch: None,
        }],
        strokes: vec![],
    };
    assert_eq!(workflow_graph_problem(&ann_graph), Some(WorkflowGraphProblem::Annotation));

    // > 200 nodes rejected
    let huge = WorkflowGraph {
        nodes: (0..=200).map(|i| node(&format!("n{i}"), WorkflowNodeKind::Prompt, 0.0, 0.0)).collect(),
        edges: vec![],
        strokes: vec![],
    };
    assert!(huge.validate().is_err());

    // SCC loop back-edge detection picks top-left entry node (`b` at x=50, y=10)
    let cyc_nodes = vec![
        node("trigger", WorkflowNodeKind::Trigger, 0.0, 0.0),
        node("b", WorkflowNodeKind::Agent, 50.0, 10.0),
        node("c", WorkflowNodeKind::Llm, 150.0, 40.0),
    ];
    let cyc_edges = vec![
        WorkflowEdge { id: "in-b".into(), from: "trigger".into(), to: "b".into(), port_type: WorkflowPortType::Text, branch: None },
        WorkflowEdge { id: "b-to-c".into(), from: "b".into(), to: "c".into(), port_type: WorkflowPortType::Text, branch: None },
        WorkflowEdge { id: "c-back-b".into(), from: "c".into(), to: "b".into(), port_type: WorkflowPortType::Text, branch: None },
    ];
    let loops = workflow_loop_edges(&cyc_nodes, &cyc_edges);
    assert!(loops.contains("c-back-b"));
    assert!(!loops.contains("b-to-c"));

    // Portable .openbot-workflow.json RAII TempDir round-trip (including required `assets` map)
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("demo.openbot-workflow.json");
    let mut assets = BTreeMap::new();
    assets.insert(
        "file-1".into(),
        WorkflowFileAsset {
            name: "logo.png".into(),
            asset_type: "image/png".into(),
            data: BASE64_STD.encode(b"png-bytes"),
        },
    );
    let wf_file = WorkflowPortableFile {
        format: WORKFLOW_FILE_FORMAT.into(),
        version: WORKFLOW_FILE_VERSION,
        graph_format: WORKFLOW_FORMAT,
        min_app_version: "1.6.15".into(),
        title: "CI Triage".into(),
        graph: g.clone(),
        references: BTreeMap::new(),
        links: vec![],
        assets,
    };
    wf_file.validate().expect("portable workflow file valid");
    std::fs::write(&path, serde_json::to_vec_pretty(&wf_file).unwrap()).unwrap();
    let loaded: WorkflowPortableFile =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(loaded, wf_file);

    // WorkflowTriggerState & WorkflowDetail wire JSON (`type`, `nextDueAt: null`, `webhookUrl: null`)
    let detail_wire = serde_json::json!({
        "summary": {
            "id": "wf1",
            "title": "Demo",
            "steps": 3,
            "updatedAt": "2026-01-01T00:00:00Z",
            "lastRun": null
        },
        "graph": serde_json::to_value(&g).unwrap(),
        "revision": 1,
        "triggers": [{
            "nodeId": "t1",
            "type": "schedule",
            "nextDueAt": null,
            "webhookUrl": null
        }]
    });
    let detail: WorkflowDetail = serde_json::from_value(detail_wire.clone()).unwrap();
    detail.validate().expect("valid WorkflowDetail");
    assert_eq!(detail.triggers[0].trigger_type, WorkflowTriggerType::Schedule);
    let reencoded = serde_json::to_value(&detail).unwrap();
    assert_eq!(reencoded["triggers"][0]["type"], "schedule");
    assert_eq!(reencoded["triggers"][0]["nextDueAt"], serde_json::Value::Null);
    assert_eq!(reencoded["triggers"][0]["webhookUrl"], serde_json::Value::Null);

    let mut bad_detail = detail.clone();
    bad_detail.needs_newer_app = Some(false);
    assert!(bad_detail.validate().is_err());
    let mut zero_fmt = detail;
    zero_fmt.format = Some(0);
    assert!(zero_fmt.validate().is_err());

    // WorkflowNode::is_valid (`schedule` only on `trigger`), WorkflowStroke::is_valid, workflow_needs_admin
    let mut trig_with_sched = node("t-s", WorkflowNodeKind::Trigger, 0.0, 0.0);
    trig_with_sched.schedule = Some(RoutineSchedule::Hourly { minute: 0 });
    assert!(trig_with_sched.is_valid());
    let mut llm_with_sched = node("l-s", WorkflowNodeKind::Llm, 0.0, 0.0);
    llm_with_sched.schedule = Some(RoutineSchedule::Hourly { minute: 0 });
    assert!(!llm_with_sched.is_valid());

    let stroke_ok = WorkflowStroke {
        id: "s1".into(),
        brush: "b".repeat(64),
        color: "c".repeat(64),
        width: 300.0,
        points: vec![],
    };
    assert!(stroke_ok.is_valid());
    assert!(!WorkflowStroke { brush: "".into(), ..stroke_ok.clone() }.is_valid());
    assert!(!WorkflowStroke { width: 100_001.0, ..stroke_ok }.is_valid());

    let mut trig_node = node("t-adm", WorkflowNodeKind::Trigger, 0.0, 0.0);
    trig_node.fields.insert("trigger".into(), WorkflowFieldValue::Text("manual".into()));
    assert!(!workflow_needs_admin(&trig_node));
    trig_node.fields.insert("trigger".into(), WorkflowFieldValue::Text("github".into()));
    assert!(workflow_needs_admin(&trig_node));
    trig_node.fields.insert("trigger".into(), WorkflowFieldValue::Text("future-unknown-trigger".into()));
    assert!(workflow_needs_admin(&trig_node));
}

#[test]
fn routines_cron_minimum_cadence_webhooks_and_crm_contracts() {
    assert!(is_valid_timezone("Europe/Warsaw"));
    assert!(is_valid_timezone("UTC"));
    assert!(!is_valid_timezone("Mars/Olympus"));

    // >= 180s (3m) minimum cadence across Interval, Advanced, and 5-field Cron
    let ok_schedules = [
        RoutineSchedule::Hourly { minute: 15 },
        RoutineSchedule::Daily { time: "09:30".into() },
        RoutineSchedule::Weekdays { time: "08:00".into() },
        RoutineSchedule::Weekly { weekday: 1, time: "10:00".into() },
        RoutineSchedule::Monthly { day: 1, time: "00:00".into() },
        RoutineSchedule::Interval { amount: 3, unit: RoutineIntervalUnit::Minutes, anchor_at: "2026-11-25T00:00:00Z".into() },
        RoutineSchedule::Advanced {
            months: vec![1, 6, 12],
            days: RoutineDaySelection::DaysOfWeek { days: vec![1, 3, 5] },
            time: RoutineTimeSelection::Every { amount: 5, unit: RoutineSubDayUnit::Minutes },
        },
        RoutineSchedule::Custom { expression: "*/3 * * * *".into() },
        RoutineSchedule::Once { date: "2026-12-01".into(), time: "14:00".into() },
    ];
    for s in &ok_schedules {
        assert!(s.validate_minimum_interval().is_ok(), "schedule {s:?} should pass");
    }
    assert!(!ok_schedules[8].is_released_schedule());

    let too_fast = [
        RoutineSchedule::Interval { amount: 2, unit: RoutineIntervalUnit::Minutes, anchor_at: "2026-11-25T00:00:00Z".into() },
        RoutineSchedule::Custom { expression: "* * * * *".into() },
        RoutineSchedule::Custom { expression: "*/2 * * * *".into() },
        RoutineSchedule::Custom { expression: "0,59 * * * *".into() },
        // Advanced hour-wraparound: [0, 29, 58] -> :58 to :00 is 120s (< 180s)
        RoutineSchedule::Advanced {
            months: vec![1],
            days: RoutineDaySelection::EveryDay,
            time: RoutineTimeSelection::Every { amount: 29, unit: RoutineSubDayUnit::Minutes },
        },
        // Advanced hour-wraparound: [0, 59] -> :59 to :00 is 60s (< 180s)
        RoutineSchedule::Advanced {
            months: vec![1],
            days: RoutineDaySelection::EveryDay,
            time: RoutineTimeSelection::Every { amount: 59, unit: RoutineSubDayUnit::Minutes },
        },
        // Impossible Feb 31 -> routineNoOccurrence
        RoutineSchedule::Custom { expression: "0 0 31 2 *".into() },
        // Non-5-field and non-decimal cron rejected by CRON_DECIMAL
        RoutineSchedule::Custom { expression: "@hourly".into() },
        RoutineSchedule::Custom { expression: "0 0 1 JAN *".into() },
    ];
    for bad in &too_fast {
        assert!(bad.validate_minimum_interval().is_err(), "schedule {bad:?} must be rejected");
    }

    // Monthly cron "0,59 0,23 1 * *" (min gap 59m) & "0/1 * * * *" (start=end=0 -> [0], once/hr) pass!
    assert!(
        RoutineSchedule::Custom {
            expression: "0,59 0,23 1 * *".into()
        }
        .validate_minimum_interval()
        .is_ok()
    );
    let zero_step = parse_cron_expression("0/1 * * * *").expect("0/1 is valid in upstream");
    assert_eq!(zero_step.minutes.into_iter().collect::<Vec<_>>(), vec![0]);
    assert!(
        RoutineSchedule::Custom {
            expression: "0/1 * * * *".into()
        }
        .validate_minimum_interval()
        .is_ok()
    );

    // RoutineFlowCanvas includes `routines: Vec<RoutineFlowRoutine>`
    let canvas = RoutineFlowCanvas {
        agent_id: "a1".into(),
        routines: vec![RoutineFlowRoutine {
            routine: RoutineFlowRoutineInfo {
                id: "r1".into(),
                agent_id: "a1".into(),
                name: "Morning Sync".into(),
                instruction: "Summarize PRs".into(),
                active: true,
                timezone: "UTC".into(),
                trigger: EventRoutineTrigger::Schedule {
                    schedule: RoutineSchedule::Hourly { minute: 0 },
                },
                limit_policy: None,
                missed_policy: None,
                run_in: None,
                paused_by_run: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            },
            recent_runs: vec![],
            upcoming_runs: vec!["2026-01-01T01:00:00Z".into()],
            steps: vec![],
        }],
        links: vec![],
        positions: vec![],
        placed_agent_ids: vec!["a1".into()],
    };
    let canvas_val = serde_json::to_value(&canvas).unwrap();
    assert_eq!(canvas_val["routines"][0]["routine"]["agentId"], "a1");
    assert_eq!(serde_json::from_value::<RoutineFlowCanvas>(canvas_val).unwrap(), canvas);

    // RFC-6901 JSON Pointer & quiet run policy
    assert!(is_event_filter_pointer(""));
    assert!(is_event_filter_pointer("/pull_request/title~0~1"));
    assert!(!is_event_filter_pointer("missing-slash"));
    assert!(!is_event_filter_pointer("/bad~escape"));
    assert!(EventFilter { pointer: "/action".into(), value: EventScalar::String("opened".into()) }.is_valid());
    assert!(quiet_routine_run(RoutineRunKind::Scheduled, None));
    assert!(quiet_routine_run(RoutineRunKind::Manual, Some(RoutineRunSource::Webhook)));
    assert!(!quiet_routine_run(RoutineRunKind::Manual, Some(RoutineRunSource::Test)));
    assert!(is_no_update_answer("  [[no-update]] "));
    assert!(is_no_update_answer("[SILENT]"));

    // Native HMAC-SHA256 `{ts}.{id}.` AND Standard Webhooks `{id}.{ts}.` `whsec_`
    let body = br#"{"event":"push"}"#;
    let ts = "1700000000";
    let input = WebhookSignatureInput {
        timestamp: ts,
        delivery_id: "del_abc-123",
        body,
        now_ms: 1_700_000_050_000,
    };
    let sig = sign_webhook_body("topsecret", ts, "del_abc-123", body);
    assert_eq!(verify_webhook_signature("topsecret", &input, &sig), Ok(()));
    assert_eq!(
        verify_webhook_signature("wrong", &input, &sig),
        Err(WebhookSecurityError::InvalidSignature)
    );
    let stale = WebhookSignatureInput { now_ms: 1_700_000_400_000, ..input.clone() };
    assert_eq!(verify_webhook_signature("topsecret", &stale, &sig), Err(WebhookSecurityError::StaleTimestamp));

    let key_bytes = [7u8; 24];
    let whsec = format!("whsec_{}", BASE64_STD.encode(key_bytes));
    let mut mac = Hmac::<Sha256>::new_from_slice(&key_bytes).unwrap();
    mac.update(b"del_abc-123.1700000000.");
    mac.update(body);
    let svix_sig = format!("v1,{}", BASE64_STD.encode(mac.finalize().into_bytes()));
    assert_eq!(verify_standard_webhook_signature(&whsec, &input, &svix_sig), Ok(()));

    // CRM bounds & 17 private fields & 10 proposed / 21 change kinds
    assert_eq!(CRM_LIMITS.amount_minor, 1_000_000_000_000);
    assert_eq!(CRM_PRIVATE_FIELDS.len(), 17);
    assert_eq!(CRM_PROPOSED_KINDS.len(), 10);
    assert_eq!(CRM_CHANGE_KINDS.len(), 21);
    assert!(is_crm_currency("USD"));
    assert!(!is_crm_currency("usd"));
    assert!(is_crm_amount(CRM_LIMITS.amount_minor));
    assert!(!is_crm_amount(CRM_LIMITS.amount_minor + 1));

    let prop = CrmProposedChange::CustomerCreate {
        customer: CrmCustomerInput { name: "Acme Corp".into(), currency: "EUR".into(), ..Default::default() },
    };
    assert!(prop.is_valid());
    let split = CrmChange::PaymentSplit {
        payment_id: "pay-1".into(),
        parts: vec![
            CrmSplitPart { amount_minor: 500, due_on: "2026-12-01".into() },
            CrmSplitPart { amount_minor: 500, due_on: "2027-01-01".into() },
        ],
    };
    assert!(split.is_valid());
    let self_replace = CrmChange::StatusDelete { status_id: "lead".into(), replace_with: "lead".into() };
    assert!(!self_replace.is_valid());

    // CrmApplyResult boolean-discriminated serde (Conflict is never shadowed by Applied!)
    let conflict_res: CrmApplyResult =
        serde_json::from_str(r#"{"ok":false,"conflict":"revision"}"#).unwrap();
    assert_eq!(
        conflict_res,
        CrmApplyResult::Conflict {
            conflict: CrmConflictReason::Revision
        }
    );
    assert_eq!(
        serde_json::to_value(&conflict_res).unwrap(),
        serde_json::json!({"ok": false, "conflict": "revision"})
    );
    let applied_null: CrmApplyResult = serde_json::from_str(r#"{"ok":true,"id":null}"#).unwrap();
    assert_eq!(applied_null, CrmApplyResult::Applied { id: None });
    assert_eq!(
        serde_json::to_value(&applied_null).unwrap(),
        serde_json::json!({"ok": true, "id": null})
    );

    // Empty patches, invalid CustomerUpdate patch, 600-char text fields, and duplicate PrivateFields rejected
    assert!(!CrmCustomerPatch::default().is_valid());
    assert!(!CrmPlanPatch::default().is_valid());
    assert!(!CrmPaymentPatch::default().is_valid());
    assert!(
        !CrmChange::CustomerUpdate {
            customer_id: "c1".into(),
            patch: CrmCustomerPatch {
                currency: Some("bad".into()),
                ..Default::default()
            },
            base_revision: 1,
        }
        .is_valid()
    );
    assert!(
        !CrmProposedChange::CustomerCreate {
            customer: CrmCustomerInput {
                name: "Acme".into(),
                currency: "USD".into(),
                contact_email: Some("x".repeat(600)),
                ..Default::default()
            },
        }
        .is_valid()
    );
    assert!(
        !CrmChange::PrivateFields {
            fields: vec![CrmPrivateField::Name, CrmPrivateField::Name],
        }
        .is_valid()
    );
}

#[test]
fn iter3_crm_strict_wire_parity_and_workflow_registry_validation() {
    use openbot_contracts::workflows::{
        WorkflowTriggerState, workflow_node_has_unknown_value, workflow_node_registry,
    };

    // 1. CRM top-level deny_unknown_fields on CrmProposedChange & CrmChange
    assert!(
        serde_json::from_str::<CrmProposedChange>(
            r#"{"kind":"customer-update","customerId":"c1","patch":{"name":"Acme"},"baseRevision":1}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<CrmChange>(r#"{"kind":"tag-create","label":"VIP","extra":true}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<CrmChange>(
            r#"{"kind":"customer-delete","customerId":"c1","force":true}"#
        )
        .is_err()
    );

    // 2. CRM non_null_option rejects explicit `null` on `?: T` fields while `double_option` accepts `null` on `?: T | null`
    assert!(
        serde_json::from_str::<CrmProposedChange>(
            r#"{"kind":"customer-update","customerId":"c1","patch":{"name":"Acme","notes":null}}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<CrmProposedChange>(
            r#"{"kind":"customer-create","customer":{"name":"Acme","currency":"USD","tagIds":null}}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<CrmProposedChange>(
            r#"{"kind":"plan-update","planId":"p1","patch":{"amountMinor":null}}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<CrmProposedChange>(
            r#"{"kind":"payment-update","paymentId":"pay1","patch":{"dueOn":null}}"#
        )
        .is_err()
    );
    let nullable_patch: CrmProposedChange = serde_json::from_str(
        r#"{"kind":"customer-update","customerId":"c1","patch":{"ownerAgentId":null,"method":null}}"#,
    )
    .expect("ownerAgentId and method are nullable in ipc-crm.ts");
    assert!(nullable_patch.is_valid());

    // 3. CRM PaymentPaid.paidAt is required `string | null` (omitted fails, null or ISO timestamp succeeds)
    assert!(
        serde_json::from_str::<CrmProposedChange>(
            r#"{"kind":"payment-paid","paymentIds":["pay1"]}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<CrmChange>(r#"{"kind":"payment-paid","paymentIds":["pay1"]}"#)
            .is_err()
    );
    let paid_null: CrmProposedChange =
        serde_json::from_str(r#"{"kind":"payment-paid","paymentIds":["pay1"],"paidAt":null}"#)
            .unwrap();
    assert!(paid_null.is_valid());
    let paid_ts: CrmChange = serde_json::from_str(
        r#"{"kind":"payment-paid","paymentIds":["pay1"],"paidAt":"2026-05-01T12:00:00Z"}"#,
    )
    .unwrap();
    assert!(paid_ts.is_valid());

    // 4. WorkflowTriggerState requires explicit `nextDueAt` and `webhookUrl` keys
    assert!(
        serde_json::from_str::<WorkflowTriggerState>(r#"{"nodeId":"t1","type":"schedule"}"#)
            .is_err()
    );

    // 5. WorkflowPortableFile::validate rejects graphFormat > WORKFLOW_FORMAT (3) and unknown constrained values
    let base_file = |graph_format: u32, llm_effort: &str| {
        let mut llm = node("l1", WorkflowNodeKind::Llm, 0.0, 0.0);
        llm.fields.insert(
            "effort".into(),
            WorkflowFieldValue::Text(llm_effort.to_string()),
        );
        WorkflowPortableFile {
            format: WORKFLOW_FILE_FORMAT.into(),
            version: WORKFLOW_FILE_VERSION,
            graph_format,
            min_app_version: "0.78.0".into(),
            title: "Portable Check".into(),
            graph: WorkflowGraph {
                nodes: vec![llm],
                edges: vec![],
                strokes: vec![],
            },
            references: BTreeMap::new(),
            links: vec![],
            assets: BTreeMap::new(),
        }
    };
    assert!(base_file(1, "high").validate().is_ok());
    assert!(base_file(WORKFLOW_FORMAT, "ultra").validate().is_ok());
    assert!(base_file(0, "high").validate().is_err());
    assert!(base_file(WORKFLOW_FORMAT + 1, "high").validate().is_err());
    assert!(base_file(WORKFLOW_FORMAT, "galactic").validate().is_err());

    // Verify all 12 constrained specs in WORKFLOW_NODE_REGISTRY have non-empty values & list newline semantics
    for (kind, field_name) in [
        (WorkflowNodeKind::Trigger, "trigger"),
        (WorkflowNodeKind::Trigger, "event"),
        (WorkflowNodeKind::Trigger, "filters"),
        (WorkflowNodeKind::Llm, "effort"),
        (WorkflowNodeKind::Condition, "effort"),
        (WorkflowNodeKind::Output, "target"),
        (WorkflowNodeKind::Output, "item"),
        (WorkflowNodeKind::Output, "action"),
        (WorkflowNodeKind::Output, "labelsFrom"),
        (WorkflowNodeKind::Output, "reason"),
        (WorkflowNodeKind::Output, "emoji"),
        (WorkflowNodeKind::Image, "media"),
        (WorkflowNodeKind::Image, "aspect"),
        (WorkflowNodeKind::Image, "background"),
        (WorkflowNodeKind::Image, "voice"),
    ] {
        let spec = workflow_node_registry(kind)
            .iter()
            .find(|s| s.name == field_name)
            .unwrap_or_else(|| panic!("missing spec {kind:?}.{field_name}"));
        assert!(
            !spec.values.is_empty(),
            "expected constrained values on {kind:?}.{field_name}"
        );
    }

    let mut trig = node("t1", WorkflowNodeKind::Trigger, 0.0, 0.0);
    trig.fields.insert(
        "filters".into(),
        WorkflowFieldValue::Text("notBot\nfirstTime".into()),
    );
    assert!(!workflow_node_has_unknown_value(&trig));
    trig.fields.insert(
        "filters".into(),
        WorkflowFieldValue::Text("notBot,firstTime".into()),
    );
    assert!(workflow_node_has_unknown_value(&trig));
}


#[test]
fn iter4_workflow_portable_graph_unknown_keys_and_routines_wire_parity() {
    use openbot_contracts::routines::{
        ChannelRoutine, ChannelRoutineRun, EventRoutine, Routine, RoutineFeed, RoutineFlowStep,
        RoutineMissedRuns, RoutinePausedByRun, RoutineResultDelivery, RoutineRun,
    };
    use openbot_contracts::workflows::workflow_graph_has_unknown_key;

    // 1. Challenger 2 Defect 2: `workflow_graph_has_unknown_key` & `WorkflowPortableFile::graph` vs permissive `WorkflowDetail::graph`
    assert!(!workflow_graph_has_unknown_key(&serde_json::Value::Null));
    assert!(!workflow_graph_has_unknown_key(&serde_json::json!({
        "nodes": [],
        "edges": [],
        "strokes": []
    })));

    let unknown_graphs = [
        serde_json::json!({"nodes": [], "edges": [], "strokes": [], "futureLayout": true}),
        serde_json::json!({
            "nodes": [{"id": "n1", "kind": "llm", "at": {"x": 0, "y": 0}, "fields": {}, "futureProp": 1}],
            "edges": [],
            "strokes": []
        }),
        serde_json::json!({
            "nodes": [{"id": "n1", "kind": "llm", "at": {"x": 0, "y": 0}, "size": {"width": 200, "futureDim": 9}, "fields": {}}],
            "edges": [],
            "strokes": []
        }),
        serde_json::json!({
            "nodes": [
                {"id": "n1", "kind": "prompt", "at": {"x": 0, "y": 0}, "fields": {}},
                {"id": "n2", "kind": "output", "at": {"x": 100, "y": 0}, "fields": {}}
            ],
            "edges": [{"id": "e1", "from": "n1", "to": "n2", "type": "text", "futureEdge": true}],
            "strokes": []
        }),
        serde_json::json!({
            "nodes": [],
            "edges": [],
            "strokes": [{"id": "s1", "brush": "pen", "color": "#fff", "width": 2, "points": [], "futureStroke": 1}]
        }),
    ];

    for bad_graph in &unknown_graphs {
        assert!(workflow_graph_has_unknown_key(bad_graph));
        let portable_raw = serde_json::json!({
            "format": WORKFLOW_FILE_FORMAT,
            "version": WORKFLOW_FILE_VERSION,
            "graphFormat": WORKFLOW_FORMAT,
            "minAppVersion": "0.78.0",
            "title": "Newer Export",
            "graph": bad_graph,
            "references": {},
            "links": [],
            "assets": {}
        });
        assert!(
            serde_json::from_value::<WorkflowPortableFile>(portable_raw).is_err(),
            "WorkflowPortableFile must reject unknown graph keys: {bad_graph}"
        );

        let detail_raw = serde_json::json!({
            "summary": {
                "id": "wf1",
                "title": "Remote Workflow",
                "steps": 1,
                "updatedAt": "2026-11-25T00:00:00Z",
                "lastRun": null
            },
            "graph": bad_graph,
            "revision": 1,
            "triggers": [],
            "needsNewerApp": true
        });
        let detail: WorkflowDetail = serde_json::from_value(detail_raw)
            .expect("WorkflowDetail::graph must stay permissive of unknown keys");
        detail.validate().expect("permissive WorkflowDetail validates");
    }

    // 2. Routines required-nullable vs optional non-nullable wire parity
    assert!(serde_json::from_str::<Routine>(
        r#"{"id":"r1","agentId":"a1","name":"R","instruction":"I","active":true,"timezone":"UTC","trigger":{"kind":"schedule","schedule":{"kind":"hourly","minute":0}},"limitPolicy":null,"createdAt":"2026-11-25T00:00:00Z","updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<ChannelRoutine>(
        r#"{"id":"r1","channelId":"ch1","name":"R","instruction":"I","active":true,"timezone":"UTC","trigger":{"kind":"schedule","schedule":{"kind":"hourly","minute":0}},"missedPolicy":null,"createdAt":"2026-11-25T00:00:00Z","updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<EventRoutine>(
        r#"{"id":"r1","owner":{"kind":"agent","id":"a1"},"name":"R","instruction":"I","active":true,"timezone":"UTC","trigger":{"kind":"webhook","url":null,"eventType":null,"filters":[]},"runIn":null,"createdAt":"2026-11-25T00:00:00Z","updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<EventRoutineTrigger>(
        r#"{"kind":"webhook","filters":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<RoutineMissedRuns>(r#"{"count":2}"#).is_err());
    assert!(!(RoutineMissedRuns { count: 0, until: None }).is_valid());
    assert!((RoutinePausedByRun { scheduled_for: "2026-11-25T00:00:00Z".into() }).is_valid());
    assert!(!(RoutinePausedByRun { scheduled_for: "not-a-date".into() }).is_valid());

    assert!(serde_json::from_str::<RoutineRun>(
        r#"{"id":"run1","routineId":"r1","agentId":"a1","kind":"scheduled","scheduledFor":"2026-11-25T00:00:00Z","routineName":"R","instruction":"I","status":"queued","createdAt":"2026-11-25T00:00:00Z","updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<ChannelRoutineRun>(
        r#"{"id":"run1","routineId":"r1","channelId":"ch1","kind":"scheduled","scheduledFor":"2026-11-25T00:00:00Z","routineName":"R","instruction":"I","status":"queued","createdAt":"2026-11-25T00:00:00Z","updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<RoutineFlowStep>(
        r#"{"id":"s1","runId":"run1","agentId":"a1","input":"in","status":"running","createdAt":"2026-11-25T00:00:00Z","updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<RoutineResultDelivery>(
        r#"{"runId":"run1","status":"pending","attempts":0,"destination":{"connectionId":"c1","platform":"telegram","connectionName":"TG","placeId":"p1","placeName":"Ops"},"updatedAt":"2026-11-25T00:00:00Z"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<RoutineFeed>(r#"{}"#).is_err());
    assert_eq!(
        serde_json::from_str::<RoutineFeed>(r#"{"url":null}"#).unwrap().url,
        None
    );
}
