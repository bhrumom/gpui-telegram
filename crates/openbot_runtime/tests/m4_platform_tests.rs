//! Comprehensive Milestone M4 (`OB-F11`, `OB-F12`, `OB-F13`) unit and live loopback HTTP/SSE
//! integration tests for `openbot_runtime::platform`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use chrono::Utc;
use hmac::{Hmac, Mac};
use openbot_contracts::agents::{AgentProviderId, CreateAgentInput, UpdateAgentInput};
use openbot_contracts::channels::{
    ChannelDraft, ChannelMember, ChannelMessage, ChannelMessageAuthor, ChannelMessageAuthorKind,
};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use openbot_contracts::routines::{
    EventFilter, EventScalar, RoutineDaySelection, RoutineIntervalUnit, RoutineLimitPolicy,
    RoutineMissedPolicy, RoutineRunKind, RoutineRunPlace, RoutineRunSource, RoutineSchedule,
    RoutineSubDayUnit, RoutineTimeSelection, sign_webhook_body,
};
use openbot_contracts::team_protocol::{
    TEAM_ABSENT_CAPABILITIES_HEADER, TEAM_APP_VERSION_HEADER, TEAM_CAPABILITIES_HEADER,
    TEAM_LONG_CAPABILITY_LISTS_CAPABILITY, TEAM_PROTOCOL_VERSION_HEADER, TeamProtocolRange,
    TeamProtocolSupport,
};
use openbot_contracts::voice::{
    CODEX_LIVE_VOICES, CodexLiveVoice, SPEECH_SPEEDS, SpeechSettings, VOICE_TURN_PAUSES,
    VoiceLiveStart, VoiceServerRoutes,
};
use openbot_contracts::workflows::{
    WorkflowBranch, WorkflowEdge, WorkflowFieldValue, WorkflowGraph, WorkflowNode,
    WorkflowNodeKind, WorkflowNodeSize, WorkflowPoint, WorkflowPortType,
};
use openbot_runtime::error::OpenBotRuntimeError;
use openbot_runtime::platform::{
    AutomationHttpServer, AutomationServerConfig, BridgeEvalRequest, CreditMediaOutcome,
    CrmProposeRequest, CrmService, CrmSyncRequest, ExternalBridgeOrchestrator,
    HtmlAndCreditMediaService, HtmlRenderRequest, JoinedServerClient, MarketplaceSearchRequest,
    MarketplaceService, MarketplaceTab, OpenBotServeConfig, OpenBotServeHandle, PlatformServices,
    RoutineEngine, RoutineRunRequest, RoutineSaveRequest, StaticSiteDeleteRequest,
    StaticSitePublishRequest, StaticSiteService, TeamApiConfig, TeamApiServer,
    TeamProtocolEvalRequest, VoiceEvalRequest, VoiceService, WebhookPostRequest, WhatsAppDecision,
    WhatsAppSenderFacts, WhatsAppSenderRules, WorkflowEngine, WorkflowRunRequest,
    WorkflowSaveRequest, ct_eq, generate_base64url_token_32,
};
use openbot_storage::{ChannelStore, ChannelUpdateChanges, OpenBotDatabase};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::Sha256;
use tempfile::TempDir;

struct TestEnv {
    _dir: TempDir,
    db: OpenBotDatabase,
}

impl TestEnv {
    fn new() -> Self {
        let dir = TempDir::new().expect("create RAII tempfile::TempDir");
        let db = OpenBotDatabase::open_in_data_dir(dir.path()).expect("open OpenBotDatabase");
        Self { _dir: dir, db }
    }

    fn path(&self) -> &Path {
        self._dir.path()
    }

    fn create_agent(&self, name: &str, allow_automation: Option<bool>) -> String {
        let store = self.db.agent_store().expect("open AgentStore");
        let seed = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let avatar_seed = if seed.is_empty() {
            "m4agent".to_string()
        } else {
            seed
        };
        let agent = store
            .create_agent(&CreateAgentInput {
                name: name.to_string(),
                description: "M4 platform integration test agent".to_string(),
                avatar_seed,
                avatar_hue: None,
                avatar_color: None,
                initial_message: "Ready for M4 checks.".to_string(),
                provider: Some(AgentProviderId::Codex),
                model: None,
                reasoning_effort: None,
                provider_account_id: None,
                operation_id: None,
            })
            .expect("create test agent");
        if allow_automation.is_some() {
            store
                .update_agent(
                    &UpdateAgentInput {
                        agent_id: agent.id.clone(),
                        allow_automation: Some(allow_automation),
                        ..Default::default()
                    },
                    None,
                )
                .expect("update agent allow_automation");
        }
        agent.id
    }

    fn create_channel(&self, name: &str, members: &[&str], lead: Option<&str>) -> String {
        let store = self.db.channel_store();
        let channel_id = format!("channel-{}", uuid::Uuid::new_v4());
        let draft = ChannelDraft {
            name: name.to_string(),
            title: String::new(),
            instructions: "M4 test channel".to_string(),
            members: members
                .iter()
                .map(|id| ChannelMember {
                    agent_id: (*id).to_string(),
                })
                .collect(),
            lead_agent_id: lead.map(str::to_string),
            isolated: None,
            browser: None,
            icon: None,
        };
        let channel = store.create(&channel_id, draft);
        let saved = store
            .update(channel, ChannelUpdateChanges::default(), None)
            .expect("persist channel");
        saved.id
    }
}

fn loopback_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback reqwest client")
}

fn hmac_sha256_bytes(key: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(payload);
    mac.finalize().into_bytes().to_vec()
}

fn standard_webhook_v1_sig(secret: &str, delivery_id: &str, ts: &str, body: &[u8]) -> String {
    let mut msg = format!("{delivery_id}.{ts}.").into_bytes();
    msg.extend_from_slice(body);
    let digest = hmac_sha256_bytes(secret.as_bytes(), &msg);
    format!("v1,{}", BASE64_STD.encode(digest))
}

fn bare_hex_sig(secret: &str, body: &[u8]) -> String {
    hex::encode(hmac_sha256_bytes(secret.as_bytes(), body))
}

fn wf_node(
    id: &str,
    kind: WorkflowNodeKind,
    x: f64,
    y: f64,
    fields: &[(&str, &str)],
) -> WorkflowNode {
    let mut map = BTreeMap::new();
    for (k, v) in fields {
        map.insert((*k).to_string(), WorkflowFieldValue::Text((*v).to_string()));
    }
    WorkflowNode {
        id: id.to_string(),
        kind,
        at: WorkflowPoint { x, y },
        size: None,
        locked: None,
        fields: map,
        schedule: None,
    }
}

fn wf_edge(
    id: &str,
    from: &str,
    to: &str,
    port_type: WorkflowPortType,
    branch: Option<WorkflowBranch>,
) -> WorkflowEdge {
    WorkflowEdge {
        id: id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        port_type,
        branch,
    }
}

// ============================================================================
// OB-F11: Routines, Quiet Settlement, await_event Waits & HMAC Webhooks
// ============================================================================

#[test]
fn test_ob_f11_routine_cadence_guard_and_once_autopause() {
    let env = TestEnv::new();
    let services = PlatformServices::open(env.db.clone(), env.path()).expect("open PlatformServices");
    let routines = &services.routines;
    let agent_id = env.create_agent("CadenceAgent", None);
    let anchor = Utc::now().to_rfc3339();

    let ok_180 = routines
        .save_routine(RoutineSaveRequest {
            owner_id: agent_id.clone(),
            routine_id: None,
            is_channel: false,
            name: "MinCadence3m".to_string(),
            instruction: "Check health every 3 minutes".to_string(),
            timezone: "UTC".to_string(),
            active: true,
            schedule: Some(RoutineSchedule::Interval {
                amount: 3,
                unit: RoutineIntervalUnit::Minutes,
                anchor_at: anchor.clone(),
            }),
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: None,
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("accept 180s interval");
    assert_eq!(ok_180.interval_seconds, Some(180));
    assert!(ok_180.active);
    assert!(ok_180.next_run_at.is_some());

    let ok_cron = routines
        .save_routine(RoutineSaveRequest {
            owner_id: agent_id.clone(),
            routine_id: None,
            is_channel: false,
            name: "Cron5m".to_string(),
            instruction: "Run every 5m".to_string(),
            timezone: "Europe/Warsaw".to_string(),
            active: true,
            schedule: Some(RoutineSchedule::Custom {
                expression: "*/5 * * * *".to_string(),
            }),
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: None,
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("accept */5 * * * * cron");
    assert_eq!(ok_cron.interval_seconds, Some(300));

    let ok_once = routines
        .save_routine(RoutineSaveRequest {
            owner_id: agent_id.clone(),
            routine_id: None,
            is_channel: false,
            name: "FutureOnce".to_string(),
            instruction: "One-shot deploy".to_string(),
            timezone: "UTC".to_string(),
            active: true,
            schedule: Some(RoutineSchedule::Once {
                date: "2099-12-31".to_string(),
                time: "14:00".to_string(),
            }),
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: None,
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("accept future once routine");
    assert!(ok_once.active);

    let rejected_schedules = [
        RoutineSchedule::Interval {
            amount: 2,
            unit: RoutineIntervalUnit::Minutes,
            anchor_at: anchor,
        },
        RoutineSchedule::Custom {
            expression: "*/2 * * * *".to_string(),
        },
        RoutineSchedule::Custom {
            expression: "0,59 * * * *".to_string(),
        },
        RoutineSchedule::Custom {
            expression: "@hourly".to_string(),
        },
        RoutineSchedule::Custom {
            expression: "0 0 31 2 *".to_string(),
        },
        RoutineSchedule::Advanced {
            months: (1..=12).collect(),
            days: RoutineDaySelection::EveryDay,
            time: RoutineTimeSelection::Every {
                amount: 29,
                unit: RoutineSubDayUnit::Minutes,
            },
        },
    ];
    for bad in rejected_schedules {
        let err = routines
            .save_routine(RoutineSaveRequest {
                owner_id: agent_id.clone(),
                routine_id: None,
                is_channel: false,
                name: "BadCadence".to_string(),
                instruction: "Should fail".to_string(),
                timezone: "UTC".to_string(),
                active: true,
                schedule: Some(bad.clone()),
                webhook_event_type: None,
                webhook_filters: Vec::new(),
                webhook_secret: None,
                limit_policy: RoutineLimitPolicy::Wait,
                missed_policy: RoutineMissedPolicy::Skip,
                run_in: RoutineRunPlace::Chat,
            })
            .expect_err("reject invalid or <180s schedule");
        assert!(matches!(err, OpenBotRuntimeError::Validation(_)), "unexpected err for {bad:?}: {err}");
    }

    let run_once = routines
        .run_routine(RoutineRunRequest {
            owner_id: agent_id.clone(),
            routine_id: ok_once.routine_id.clone(),
            is_channel: false,
            kind: RoutineRunKind::Scheduled,
            source: None,
            payload: None,
            wait_id: None,
            quiet_no_update: false,
            report_text: Some("Deployed once".to_string()),
            await_events: Vec::new(),
        })
        .expect("run scheduled once routine");
    assert_eq!(run_once.status, "completed");
    assert!(!run_once.routine_active, "once routine must auto-pause after scheduled run");

    let stored = routines.list_routines(&agent_id, false).expect("list routines");
    let once_row = stored
        .iter()
        .find(|r| r.id == ok_once.routine_id)
        .expect("stored once routine exists");
    assert!(!once_row.active);
}

#[test]
fn test_ob_f11_routine_quiet_no_update_and_await_event_20_cap() {
    let env = TestEnv::new();
    let routines = RoutineEngine::new(env.db.clone());
    let agent_id = env.create_agent("QuietWaitAgent", None);

    let saved = routines
        .save_routine(RoutineSaveRequest {
            owner_id: agent_id.clone(),
            routine_id: None,
            is_channel: false,
            name: "QuietMonitor".to_string(),
            instruction: "Monitor quietly".to_string(),
            timezone: "UTC".to_string(),
            active: true,
            schedule: Some(RoutineSchedule::Interval {
                amount: 5,
                unit: RoutineIntervalUnit::Minutes,
                anchor_at: Utc::now().to_rfc3339(),
            }),
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: None,
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("save monitor routine");

    let quiet = routines
        .run_routine(RoutineRunRequest {
            owner_id: agent_id.clone(),
            routine_id: saved.routine_id.clone(),
            is_channel: false,
            kind: RoutineRunKind::Scheduled,
            source: None,
            payload: None,
            wait_id: None,
            quiet_no_update: true,
            report_text: Some("[[no-update]]".to_string()),
            await_events: Vec::new(),
        })
        .expect("quiet scheduled run");
    assert_eq!(quiet.status, "quiet_suppressed");
    assert!(!quiet.notified);
    assert_eq!(quiet.quiet_item_type.as_deref(), Some("routine_quiet_answer"));

    let manual = routines
        .run_routine(RoutineRunRequest {
            owner_id: agent_id.clone(),
            routine_id: saved.routine_id.clone(),
            is_channel: false,
            kind: RoutineRunKind::Manual,
            source: Some(RoutineRunSource::Test),
            payload: None,
            wait_id: None,
            quiet_no_update: true,
            report_text: Some("Manual test verification".to_string()),
            await_events: Vec::new(),
        })
        .expect("manual test run");
    assert!(manual.notified, "manual Test run must notify");

    let mut first_wait_id = String::new();
    for i in 0..20 {
        let w = routines
            .open_wait(&agent_id, &saved.routine_id, &format!("wait-event-{i}"), None)
            .expect("open wait within 20 cap");
        if i == 0 {
            first_wait_id = w.id;
        }
    }
    assert_eq!(routines.list_waits(&agent_id).expect("list waits").len(), 20);

    let err_21 = routines
        .open_wait(&agent_id, &saved.routine_id, "wait-event-21", None)
        .expect_err("21st wait must fail with LimitExceeded");
    assert!(
        matches!(err_21, OpenBotRuntimeError::LimitExceeded(_)),
        "expected LimitExceeded on 21st wait, got {err_21:?}"
    );
    assert!(routines.close_wait(&agent_id, &first_wait_id).expect("close wait"));
    assert_eq!(routines.list_waits(&agent_id).expect("list waits after close").len(), 19);
}

#[test]
fn test_ob_f11_webhook_hmac_standard_bare_filters_and_rejections() {
    let env = TestEnv::new();
    let routines = RoutineEngine::new(env.db.clone());
    let agent_id = env.create_agent("WebhookAgent", None);
    let secret = "whsec_dGVzdF9zZWNyZXRfa2V5XzE2Yl8xMjM0NTY=";

    let hook = routines
        .save_routine(RoutineSaveRequest {
            owner_id: agent_id.clone(),
            routine_id: None,
            is_channel: false,
            name: "IssueOpenedHook".to_string(),
            instruction: "Handle opened issue".to_string(),
            timezone: "UTC".to_string(),
            active: true,
            schedule: None,
            webhook_event_type: None,
            webhook_filters: vec![EventFilter {
                pointer: "/action".to_string(),
                value: EventScalar::String("opened".to_string()),
            }],
            webhook_secret: Some(secret.to_string()),
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("save webhook routine");
    assert!(hook.webhook_route_id.is_some());

    let now_ms = Utc::now().timestamp_millis();
    let ts_sec = (now_ms / 1_000).to_string();
    let body_match = br#"{"action":"opened","issue":{"id":501}}"#;

    // 1. OpenBot sha256=<hex> over {ts}.{delivery_id}.{body} + matching RFC-6901 pointer filter
    let sig_openbot = sign_webhook_body(secret, &ts_sec, "deliv-openbot-1", body_match);
    let out_match = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: hook.routine_id.clone(),
            secret: secret.to_string(),
            body: body_match.to_vec(),
            signature: sig_openbot,
            timestamp: Some(ts_sec.clone()),
            delivery_id: Some("deliv-openbot-1".to_string()),
            event_type: Some("issue.opened".to_string()),
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect("valid OpenBot HMAC webhook");
    assert!(out_match.webhook_verified);
    assert!(out_match.matched_filters);
    assert_eq!(out_match.status, "started");
    assert!(out_match.run_id.is_some());

    // 2. RFC-6901 pointer filter mismatch -> verified, ignored, no run
    let body_miss = br#"{"action":"closed","issue":{"id":502}}"#;
    let sig_miss = sign_webhook_body(secret, &ts_sec, "deliv-openbot-2", body_miss);
    let out_miss = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: hook.routine_id.clone(),
            secret: secret.to_string(),
            body: body_miss.to_vec(),
            signature: sig_miss,
            timestamp: Some(ts_sec.clone()),
            delivery_id: Some("deliv-openbot-2".to_string()),
            event_type: Some("issue.closed".to_string()),
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect("valid sig with non-matching filter");
    assert!(out_miss.webhook_verified);
    assert!(!out_miss.matched_filters);
    assert_eq!(out_miss.status, "ignored");
    assert!(out_miss.run_id.is_none());

    // 3. Standard Webhooks v1,<base64> over {delivery_id}.{ts}.{body}
    let sig_std = standard_webhook_v1_sig(secret, "deliv-std-1", &ts_sec, body_match);
    let out_std = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: hook.routine_id.clone(),
            secret: secret.to_string(),
            body: body_match.to_vec(),
            signature: sig_std,
            timestamp: Some(ts_sec.clone()),
            delivery_id: Some("deliv-std-1".to_string()),
            event_type: Some("issue.opened".to_string()),
            standard_webhooks: true,
            now_ms: Some(now_ms),
        })
        .expect("valid Standard Webhooks v1,<b64>");
    assert!(out_std.webhook_verified);
    assert!(out_std.matched_filters);
    assert_eq!(out_std.status, "started");

    // 4. Bare 64-hex HMAC-SHA256 over raw body
    let sig_bare = bare_hex_sig(secret, body_match);
    let out_bare = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: hook.routine_id.clone(),
            secret: secret.to_string(),
            body: body_match.to_vec(),
            signature: sig_bare,
            timestamp: None,
            delivery_id: None,
            event_type: Some("issue.opened".to_string()),
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect("valid bare 64-hex HMAC");
    assert!(out_bare.webhook_verified);
    assert!(out_bare.matched_filters);

    // 5. Adversarial rejections on a fresh routine -> zero runs inserted
    let adv_secret = "wh_guard_secret_42";
    let adv_hook = routines
        .save_routine(RoutineSaveRequest {
            owner_id: agent_id.clone(),
            routine_id: None,
            is_channel: false,
            name: "GuardedHook".to_string(),
            instruction: "Guarded".to_string(),
            timezone: "UTC".to_string(),
            active: true,
            schedule: None,
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: Some(adv_secret.to_string()),
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("save guarded webhook routine");
    let raw_body = br#"{"event":"push"}"#;

    // Tampered signature
    let err_tampered = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: adv_hook.routine_id.clone(),
            secret: adv_secret.to_string(),
            body: raw_body.to_vec(),
            signature: format!("sha256={}", "0".repeat(64)),
            timestamp: Some(ts_sec.clone()),
            delivery_id: Some("deliv-bad-1".to_string()),
            event_type: None,
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect_err("reject tampered signature");
    assert!(matches!(err_tampered, OpenBotRuntimeError::Unauthorized(_)));

    // > 300s stale timestamp
    let stale_ts = ((now_ms / 1_000) - 301).to_string();
    let stale_sig = sign_webhook_body(adv_secret, &stale_ts, "deliv-stale-1", raw_body);
    let err_stale = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: adv_hook.routine_id.clone(),
            secret: adv_secret.to_string(),
            body: raw_body.to_vec(),
            signature: stale_sig,
            timestamp: Some(stale_ts),
            delivery_id: Some("deliv-stale-1".to_string()),
            event_type: None,
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect_err("reject >300s stale timestamp");
    assert!(matches!(
        err_stale,
        OpenBotRuntimeError::Unauthorized(_) | OpenBotRuntimeError::Validation(_)
    ));

    // Malformed delivery_id
    let bad_did_sig = sign_webhook_body(adv_secret, &ts_sec, "bad id!!", raw_body);
    let _ = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id.clone(),
            routine_id: adv_hook.routine_id.clone(),
            secret: adv_secret.to_string(),
            body: raw_body.to_vec(),
            signature: bad_did_sig,
            timestamp: Some(ts_sec.clone()),
            delivery_id: Some("bad id!!".to_string()),
            event_type: None,
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect_err("reject malformed delivery_id");

    // > 64 KiB body
    let big_body = vec![b'Z'; 65_537];
    let big_sig = sign_webhook_body(adv_secret, &ts_sec, "deliv-big-1", &big_body);
    let _ = routines
        .post_webhook(WebhookPostRequest {
            owner_id: agent_id,
            routine_id: adv_hook.routine_id.clone(),
            secret: adv_secret.to_string(),
            body: big_body,
            signature: big_sig,
            timestamp: Some(ts_sec),
            delivery_id: Some("deliv-big-1".to_string()),
            event_type: None,
            standard_webhooks: false,
            now_ms: Some(now_ms),
        })
        .expect_err("reject >64 KiB body");

    let inserted_runs: i64 = env
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM projection_routine_runs WHERE routine_id = ?1",
                rusqlite::params![adv_hook.routine_id],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .expect("query routine runs count");
    assert_eq!(inserted_runs, 0, "rejected webhooks must insert zero runs");
}

// ============================================================================
// OB-F11: 12-Node Visual DAG WorkflowEngine
// ============================================================================

#[test]
fn test_ob_f11_workflow_12_node_kinds_and_graph_validation_guards() {
    let env = TestEnv::new();
    let engine = WorkflowEngine::new(env.db.clone());
    let agent_id = env.create_agent("WfAgent", None);
    let channel_id = env.create_channel("WfRoom", &[&agent_id], Some(&agent_id));

    let mut shape_node = wf_node("n_shape", WorkflowNodeKind::Shape, 600.0, -180.0, &[("text", "Gate Zone")]);
    shape_node.size = Some(WorkflowNodeSize {
        width: Some(200.0),
        height: Some(100.0),
        font: None,
    });

    let graph_12 = WorkflowGraph {
        nodes: vec![
            wf_node("n_trig", WorkflowNodeKind::Trigger, 0.0, 0.0, &[("trigger", "manual"), ("stepName", "Start")]),
            wf_node("n_prompt", WorkflowNodeKind::Prompt, 120.0, -80.0, &[("text", "Summarize PR")]),
            wf_node("n_asset", WorkflowNodeKind::Asset, 120.0, 80.0, &[("fileId", "asset-spec-01")]),
            wf_node("n_llm", WorkflowNodeKind::Llm, 280.0, 0.0, &[("provider", "codex"), ("model", "gpt-6-luna"), ("prompt", "Classify")]),
            wf_node("n_agent", WorkflowNodeKind::Agent, 440.0, 0.0, &[("agentId", &agent_id), ("task", "Review diff")]),
            wf_node("n_img", WorkflowNodeKind::Image, 440.0, 140.0, &[("media", "image"), ("model", "gpt-image"), ("prompt", "Status card")]),
            wf_node("n_cond", WorkflowNodeKind::Condition, 600.0, 0.0, &[("rule", "approved == true")]),
            wf_node("n_appr", WorkflowNodeKind::Approval, 760.0, -60.0, &[("question", "Ship release?")]),
            wf_node("n_out", WorkflowNodeKind::Output, 920.0, 0.0, &[("target", "channel"), ("channelId", &channel_id), ("message", "Done")]),
            wf_node("n_txt", WorkflowNodeKind::Text, 0.0, -180.0, &[("text", "Pipeline Header")]),
            wf_node("n_note", WorkflowNodeKind::Note, 280.0, -180.0, &[("text", "Sticky note")]),
            shape_node,
        ],
        edges: vec![
            wf_edge("e1", "n_trig", "n_llm", WorkflowPortType::Text, None),
            wf_edge("e2", "n_prompt", "n_llm", WorkflowPortType::Text, None),
            wf_edge("e3", "n_asset", "n_llm", WorkflowPortType::File, None),
            wf_edge("e4", "n_llm", "n_agent", WorkflowPortType::Text, None),
            wf_edge("e5", "n_agent", "n_cond", WorkflowPortType::Text, None),
            wf_edge("e6", "n_agent", "n_img", WorkflowPortType::Text, None),
            wf_edge("e7", "n_cond", "n_appr", WorkflowPortType::Text, Some(WorkflowBranch::Yes)),
            wf_edge("e8", "n_cond", "n_out", WorkflowPortType::Text, Some(WorkflowBranch::No)),
            wf_edge("e9", "n_appr", "n_out", WorkflowPortType::Text, None),
            wf_edge("e10", "n_img", "n_out", WorkflowPortType::Image, None),
        ],
        strokes: Vec::new(),
    };

    let saved = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "All-12-Kinds-DAG".to_string(),
            graph: graph_12,
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect("save 12-node workflow");
    assert_eq!(saved.node_count, 12);
    assert_eq!(saved.step_count, 9);
    assert_eq!(saved.edge_count, 10);
    assert!(saved.loop_edges.is_empty());
    assert!(engine.get_workflow(&saved.workflow_id).expect("get workflow").is_some());
    assert!(engine.list_workflows().expect("list workflows").iter().any(|w| w.id == saved.workflow_id));

    // Cycle rejected when allow_loops == false, accepted when allow_loops == true
    let cycle_graph = WorkflowGraph {
        nodes: vec![
            wf_node("a", WorkflowNodeKind::Agent, 0.0, 0.0, &[("agentId", &agent_id), ("task", "A")]),
            wf_node("b", WorkflowNodeKind::Llm, 240.0, 0.0, &[("prompt", "B")]),
        ],
        edges: vec![
            wf_edge("e1", "a", "b", WorkflowPortType::Text, None),
            wf_edge("e2", "b", "a", WorkflowPortType::Text, None),
        ],
        strokes: Vec::new(),
    };
    let _ = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "CycleRejected".to_string(),
            graph: cycle_graph.clone(),
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect_err("reject cycle when allow_loops == false");
    let loop_ok = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "CycleAllowed".to_string(),
            graph: cycle_graph,
            raw_json_bytes: None,
            allow_loops: true,
            base_revision: None,
        })
        .expect("accept cycle when allow_loops == true");
    assert!(!loop_ok.loop_edges.is_empty());

    // > 200 nodes rejected
    let nodes_201: Vec<WorkflowNode> = (0..201)
        .map(|i| wf_node(&format!("nd_{i}"), WorkflowNodeKind::Llm, (i * 10) as f64, 0.0, &[("prompt", "step")]))
        .collect();
    let _ = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "TooManyNodes".to_string(),
            graph: WorkflowGraph { nodes: nodes_201, edges: Vec::new(), strokes: Vec::new() },
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect_err("reject >200 nodes");

    // > 400 edges rejected
    let nodes_30: Vec<WorkflowNode> = (0..30)
        .map(|i| wf_node(&format!("m_{i}"), WorkflowNodeKind::Llm, (i * 40) as f64, 0.0, &[("prompt", "M")]))
        .collect();
    let mut edges_401 = Vec::new();
    'outer: for i in 0..30 {
        for j in (i + 1)..30 {
            edges_401.push(wf_edge(
                &format!("e_{}", edges_401.len()),
                &format!("m_{i}"),
                &format!("m_{j}"),
                WorkflowPortType::Text,
                None,
            ));
            if edges_401.len() == 401 {
                break 'outer;
            }
        }
    }
    let _ = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "TooManyEdges".to_string(),
            graph: WorkflowGraph { nodes: nodes_30, edges: edges_401, strokes: Vec::new() },
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect_err("reject >400 edges");

    // Edge touching annotation rejected
    let _ = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "AnnotationEdge".to_string(),
            graph: WorkflowGraph {
                nodes: vec![
                    wf_node("note1", WorkflowNodeKind::Note, 0.0, 0.0, &[("text", "memo")]),
                    wf_node("l1", WorkflowNodeKind::Llm, 200.0, 0.0, &[("prompt", "run")]),
                ],
                edges: vec![wf_edge("e1", "note1", "l1", WorkflowPortType::Text, None)],
                strokes: Vec::new(),
            },
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect_err("reject edge touching annotation node");

    // Condition edge missing branch rejected
    let _ = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "MissingBranch".to_string(),
            graph: WorkflowGraph {
                nodes: vec![
                    wf_node("c1", WorkflowNodeKind::Condition, 0.0, 0.0, &[("rule", "ok")]),
                    wf_node("o1", WorkflowNodeKind::Output, 200.0, 0.0, &[("target", "notify"), ("message", "done")]),
                ],
                edges: vec![wf_edge("e1", "c1", "o1", WorkflowPortType::Text, None)],
                strokes: Vec::new(),
            },
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect_err("reject Condition edge missing branch");
}

#[test]
fn test_ob_f11_workflow_condition_branch_approval_pause_resume_reject() {
    let env = TestEnv::new();
    let engine = WorkflowEngine::new(env.db.clone());
    let agent_id = env.create_agent("WfApprovalAgent", None);
    let channel_id = env.create_channel("WfAlerts", &[&agent_id], Some(&agent_id));

    let graph = WorkflowGraph {
        nodes: vec![
            wf_node("t1", WorkflowNodeKind::Trigger, 0.0, 0.0, &[("trigger", "manual")]),
            wf_node("c1", WorkflowNodeKind::Condition, 220.0, 0.0, &[("rule", "pr.draft == false")]),
            wf_node("app1", WorkflowNodeKind::Approval, 440.0, -60.0, &[("question", "Approve release?")]),
            wf_node("out_yes", WorkflowNodeKind::Output, 660.0, -60.0, &[("target", "notify"), ("message", "Approved release")]),
            wf_node("out_no", WorkflowNodeKind::Output, 660.0, 80.0, &[("target", "channel"), ("channelId", &channel_id), ("message", "Draft skipped")]),
        ],
        edges: vec![
            wf_edge("e1", "t1", "c1", WorkflowPortType::Text, None),
            wf_edge("e2", "c1", "app1", WorkflowPortType::Text, Some(WorkflowBranch::Yes)),
            wf_edge("e3", "c1", "out_no", WorkflowPortType::Text, Some(WorkflowBranch::No)),
            wf_edge("e4", "app1", "out_yes", WorkflowPortType::Text, None),
        ],
        strokes: Vec::new(),
    };
    let saved = engine
        .save_workflow(WorkflowSaveRequest {
            workflow_id: None,
            title: "PR-Approval-Gate".to_string(),
            graph,
            raw_json_bytes: None,
            allow_loops: false,
            base_revision: None,
        })
        .expect("save approval workflow");

    let mut yes_mocks = BTreeMap::new();
    yes_mocks.insert("c1".to_string(), "yes".to_string());

    // Branch YES -> pauses at approval hold -> approve resumes to completed with outputs
    let paused = engine
        .run_workflow(WorkflowRunRequest {
            workflow_id: saved.workflow_id.clone(),
            run_id: None,
            payload: Some(r#"{"pr":88,"draft":false}"#.to_string()),
            approve_node: None,
            reject_node: None,
            condition_mocks: yes_mocks.clone(),
            idempotency_key: Some("run-yes-approve-1".to_string()),
        })
        .expect("start workflow on yes branch");
    assert_eq!(paused.status, "awaiting_approval");
    assert!(!paused.steps.iter().any(|s| s.node_id == "out_yes" && s.status == "completed"));

    let resumed = engine
        .decide_approval(&paused.run_id, "app1", true, &yes_mocks)
        .expect("approve workflow hold");
    assert_eq!(resumed.status, "completed");
    assert!(!resumed.outputs.is_empty());

    // Branch YES -> pauses at approval hold -> reject transitions to stopped
    let paused_reject = engine
        .run_workflow(WorkflowRunRequest {
            workflow_id: saved.workflow_id.clone(),
            run_id: None,
            payload: Some(r#"{"pr":89,"draft":false}"#.to_string()),
            approve_node: None,
            reject_node: None,
            condition_mocks: yes_mocks.clone(),
            idempotency_key: Some("run-yes-reject-1".to_string()),
        })
        .expect("start second workflow on yes branch");
    assert_eq!(paused_reject.status, "awaiting_approval");

    let rejected = engine
        .decide_approval(&paused_reject.run_id, "app1", false, &yes_mocks)
        .expect("reject workflow hold");
    assert_eq!(rejected.status, "stopped");

    // Branch NO -> bypasses approval and completes directly
    let mut no_mocks = BTreeMap::new();
    no_mocks.insert("c1".to_string(), "no".to_string());
    let no_out = engine
        .run_workflow(WorkflowRunRequest {
            workflow_id: saved.workflow_id,
            run_id: None,
            payload: Some(r#"{"pr":90,"draft":true}"#.to_string()),
            approve_node: None,
            reject_node: None,
            condition_mocks: no_mocks,
            idempotency_key: Some("run-no-direct-1".to_string()),
        })
        .expect("run workflow on no branch");
    assert_eq!(no_out.status, "completed");
    assert!(!no_out.outputs.is_empty());
}

// ============================================================================
// OB-F11: Live Loopback AutomationHttpServer
// ============================================================================

#[tokio::test]
async fn test_ob_f11_live_automation_http_server_permissions_guards_and_rate_limit() {
    let env = TestEnv::new();
    let routines = RoutineEngine::new(env.db.clone());
    let denied_agent = env.create_agent("DeniedAuto", Some(false));
    let allowed_agent = env.create_agent("AllowedAuto", Some(true));

    let rt = routines
        .save_routine(RoutineSaveRequest {
            owner_id: allowed_agent.clone(),
            routine_id: None,
            is_channel: false,
            name: "LocalScriptRoutine".to_string(),
            instruction: "Execute local script event".to_string(),
            timezone: "UTC".to_string(),
            active: true,
            schedule: Some(RoutineSchedule::Interval {
                amount: 3,
                unit: RoutineIntervalUnit::Minutes,
                anchor_at: Utc::now().to_rfc3339(),
            }),
            webhook_event_type: None,
            webhook_filters: Vec::new(),
            webhook_secret: None,
            limit_policy: RoutineLimitPolicy::Wait,
            missed_policy: RoutineMissedPolicy::Skip,
            run_in: RoutineRunPlace::Chat,
        })
        .expect("save automation routine");

    let handle = AutomationHttpServer::start(
        env.db.clone(),
        routines.clone(),
        AutomationServerConfig {
            bind_port: 0,
            data_dir: env.path().to_path_buf(),
        },
    )
    .await
    .expect("start AutomationHttpServer on 127.0.0.1:0");

    assert!(handle.addr().ip().is_loopback());
    let auto_dir = handle.automation_dir();
    assert!(auto_dir.is_dir());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&auto_dir).expect("stat automation dir").permissions().mode() & 0o777, 0o700);
        for fname in ["token", "headers", "url"] {
            let p = auto_dir.join(fname);
            assert!(p.is_file(), "missing automation file {fname}");
            assert_eq!(fs::metadata(&p).expect("stat file").permissions().mode() & 0o777, 0o600);
        }
    }
    assert_eq!(fs::read_to_string(auto_dir.join("token")).expect("read token").trim(), handle.token());

    let client = loopback_http_client();
    let run_url = format!("{}/run", handle.url());
    let bearer = format!("Bearer {}", handle.token());
    let valid_body = json!({
        "agentId": allowed_agent,
        "agent_id": allowed_agent,
        "routineId": rt.routine_id,
        "routine_id": rt.routine_id,
        "payload": "check health",
    });

    // Wrong Host -> 403
    let resp_host = client
        .post(&run_url)
        .header(reqwest::header::HOST, "evil.rebind.example")
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .json(&valid_body)
        .send()
        .await
        .expect("wrong host request");
    assert_eq!(resp_host.status(), StatusCode::FORBIDDEN);

    // Present Origin -> 403
    let resp_origin = client
        .post(&run_url)
        .header(reqwest::header::ORIGIN, "https://evil.example")
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .json(&valid_body)
        .send()
        .await
        .expect("origin request");
    assert_eq!(resp_origin.status(), StatusCode::FORBIDDEN);

    // Missing Bearer -> 401; wrong Bearer -> 401
    let resp_no_auth = client.post(&run_url).json(&valid_body).send().await.expect("no auth");
    assert_eq!(resp_no_auth.status(), StatusCode::UNAUTHORIZED);
    let resp_bad_auth = client
        .post(&run_url)
        .header(reqwest::header::AUTHORIZATION, "Bearer wrong-secret-token")
        .json(&valid_body)
        .send()
        .await
        .expect("wrong auth");
    assert_eq!(resp_bad_auth.status(), StatusCode::UNAUTHORIZED);

    // allow_automation=false agent -> 403
    let resp_denied = client
        .post(&run_url)
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .json(&json!({ "agentId": denied_agent, "agent_id": denied_agent, "payload": "ping" }))
        .send()
        .await
        .expect("denied agent");
    assert_eq!(resp_denied.status(), StatusCode::FORBIDDEN);

    // 4,001-char payload -> 413
    let resp_413 = client
        .post(&run_url)
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .json(&json!({
            "agentId": allowed_agent,
            "agent_id": allowed_agent,
            "routineId": rt.routine_id,
            "routine_id": rt.routine_id,
            "payload": "P".repeat(4_001),
        }))
        .send()
        .await
        .expect("oversized payload");
    assert_eq!(resp_413.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // 30 valid runs -> 202
    for i in 0..30 {
        let resp_ok = client
            .post(&run_url)
            .header(reqwest::header::AUTHORIZATION, &bearer)
            .json(&json!({
                "agentId": allowed_agent,
                "agent_id": allowed_agent,
                "routineId": rt.routine_id,
                "routine_id": rt.routine_id,
                "payload": format!("run-{i}"),
            }))
            .send()
            .await
            .expect("valid automation run");
        assert_eq!(resp_ok.status(), StatusCode::ACCEPTED, "run #{i} should return 202");
    }

    // 31st run -> 429 with limit_per_hour == 30
    let resp_31 = client
        .post(&run_url)
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .json(&valid_body)
        .send()
        .await
        .expect("31st automation run");
    assert_eq!(resp_31.status(), StatusCode::TOO_MANY_REQUESTS);
    let body_31: Value = resp_31.json().await.expect("parse 429 JSON");
    assert_eq!(
        body_31
            .get("limit_per_hour")
            .or_else(|| body_31.get("limitPerHour"))
            .and_then(Value::as_u64),
        Some(30)
    );

    // Clean shutdown removes token/headers/url files
    handle.shutdown().await.expect("shutdown AutomationHttpServer");
    assert!(!auto_dir.join("token").exists());
    assert!(!auto_dir.join("headers").exists());
    assert!(!auto_dir.join("url").exists());
}

// ============================================================================
// OB-F12: CRM, Static Sites, Marketplace, Sandboxed HTML & Credit Media
// ============================================================================

#[test]
fn test_ob_f12_crm_13_tables_stripe_guards_proposals_cap_and_redaction() {
    let env = TestEnv::new();
    let crm = CrmService::open(env.db.clone(), env.path()).expect("open CrmService");
    assert!(crm.set_enabled(true).expect("enable CRM").enabled);
    assert!(crm.settings().expect("read CRM settings").enabled);

    // 1. Verify all 13 CRM SQLite tables exist
    let crm_table_count: i64 = env
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'crm_%'",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .expect("count crm_* tables");
    assert_eq!(crm_table_count, 13);

    // 2. Stripe sk_live_* secret key rejected with sourceSecretKey
    let sk_err = crm
        .sync_stripe(CrmSyncRequest {
            source: "stripe".to_string(),
            api_key: Some("sk_live_9999999999".to_string()),
            ..Default::default()
        })
        .expect_err("reject sk_live_* secret key");
    assert!(
        sk_err.to_string().to_ascii_lowercase().contains("sourcesecretkey"),
        "expected sourceSecretKey error, got: {sk_err}"
    );

    // 3. Valid restricted key + Stripe fixture sync
    let fixture = json!({
        "customers": [
            { "id": "stripe:cus_101", "name": "Acme Robotics", "email": "ops@acme.test", "status": "customer" }
        ],
        "plans": [
            { "id": "stripe:sub_201", "customerId": "stripe:cus_101", "amountMinor": 49000, "currency": "USD" }
        ],
        "payments": [
            { "id": "stripe:in_301", "customerId": "stripe:cus_101", "amountMinor": 49000, "currency": "USD" }
        ]
    });
    let sync_out = crm
        .sync_stripe(CrmSyncRequest {
            source: "stripe".to_string(),
            api_key: Some("rk_test_restricted_valid_01".to_string()),
            fixture_json: Some(fixture),
            ..Default::default()
        })
        .expect("sync Stripe fixture");
    assert_eq!(sync_out.tables_count, 13);
    assert_eq!(sync_out.builtin_statuses, vec!["lead", "customer", "former"]);
    assert_eq!(sync_out.synced_customers, 1);
    assert_eq!(sync_out.synced_plans, 1);
    assert_eq!(sync_out.synced_payments, 1);

    let aid = env.create_agent("CrmAgent", None);
    let stripe_cid: String = env
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT customer_id FROM crm_customers WHERE source_id IS NOT NULL LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .expect("find synced Stripe customer");

    // 4. sourceOwned proposal rejected (linked Stripe customer ID mutation)
    let src_err = crm
        .propose_or_apply(CrmProposeRequest {
            agent_id: Some(aid.clone()),
            operation: Some("customer-update".to_string()),
            payload: Some(json!({
                "customerId": stripe_cid,
                "patch": {
                    "name": "Tampered Stripe Name",
                    "stripeCustomerId": "cus_hijacked"
                }
            })),
            reason: Some("Attempt sourceOwned edit".to_string()),
            ..Default::default()
        })
        .expect_err("reject sourceOwned proposal");
    assert!(
        src_err.to_string().to_ascii_lowercase().contains("sourceowned"),
        "expected sourceOwned error, got: {src_err}"
    );

    // 5. --commit-unapproved forbidden
    let unapproved_err = crm
        .propose_or_apply(CrmProposeRequest {
            agent_id: Some(aid.clone()),
            operation: Some("customer-create".to_string()),
            payload: Some(json!({ "name": "Bypass Corp", "currency": "USD", "statusId": "lead" })),
            reason: Some("Try bypass".to_string()),
            commit_unapproved: true,
            ..Default::default()
        })
        .expect_err("reject commit_unapproved");
    assert!(matches!(unapproved_err, OpenBotRuntimeError::Forbidden(_)));

    // 6. builtinStatus delete & rename rejected
    let del_status_err = crm
        .propose_or_apply(CrmProposeRequest {
            direct_change: Some(json!({
                "kind": "status-delete",
                "statusId": "lead",
                "replaceWith": "customer"
            })),
            ..Default::default()
        })
        .expect_err("reject builtin status delete");
    assert!(
        del_status_err.to_string().to_ascii_lowercase().contains("builtinstatus"),
        "expected builtinStatus error, got: {del_status_err}"
    );
    let ren_status_err = crm
        .propose_or_apply(CrmProposeRequest {
            direct_change: Some(json!({
                "kind": "status-rename",
                "statusId": "lead",
                "label": "Prospect"
            })),
            ..Default::default()
        })
        .expect_err("reject builtin status rename");
    assert!(
        ren_status_err.to_string().to_ascii_lowercase().contains("builtinstatus"),
        "expected builtinStatus error, got: {ren_status_err}"
    );

    // 7. Manual customer + payment -> PaymentSplit splitTotal mismatch rejected
    let c_prop = crm
        .propose_or_apply(CrmProposeRequest {
            agent_id: Some(aid.clone()),
            operation: Some("customer-create".to_string()),
            payload: Some(json!({
                "name": "Nordic Labs",
                "currency": "USD",
                "statusId": "customer",
                "contactName": "Ada",
                "contactEmail": "ada@nordic.test",
                "phone": "+461234567",
                "billingEmail": "cfo@nordic.test",
                "domain": "nordic.test",
                "taxId": "EU-998877",
                "address": "Stockholm 1",
                "notes": "Confidential tier"
            })),
            reason: Some("Add Nordic Labs".to_string()),
            ..Default::default()
        })
        .expect("propose manual customer");
    assert_eq!(c_prop.status, "pending_approval");
    assert!(c_prop.approval_required);
    let c_committed = crm
        .propose_or_apply(CrmProposeRequest {
            proposal_id: c_prop.proposal_id,
            approve: true,
            ..Default::default()
        })
        .expect("approve manual customer");
    assert_eq!(c_committed.status, "committed");
    let manual_cid = c_committed.committed_id.expect("committed customer id");

    let p_prop = crm
        .propose_or_apply(CrmProposeRequest {
            agent_id: Some(aid.clone()),
            operation: Some("payment-create".to_string()),
            payload: Some(json!({
                "customerId": manual_cid,
                "label": "Annual Invoice",
                "amountMinor": 10_000,
                "dueOn": "2026-12-01"
            })),
            reason: Some("Add invoice".to_string()),
            ..Default::default()
        })
        .expect("propose payment");
    let p_committed = crm
        .propose_or_apply(CrmProposeRequest {
            proposal_id: p_prop.proposal_id,
            approve: true,
            ..Default::default()
        })
        .expect("approve payment");
    let pay_id = p_committed.committed_id.expect("committed payment id");

    let split_err = crm
        .propose_or_apply(CrmProposeRequest {
            agent_id: Some(aid.clone()),
            operation: Some("payment-split".to_string()),
            payload: Some(json!({
                "paymentId": pay_id,
                "parts": [
                    { "amountMinor": 4_000, "dueOn": "2026-12-15" },
                    { "amountMinor": 4_000, "dueOn": "2026-12-31" }
                ]
            })),
            reason: Some("Mismatched split".to_string()),
            ..Default::default()
        })
        .expect_err("reject PaymentSplit splitTotal mismatch");
    assert!(
        split_err.to_string().to_ascii_lowercase().contains("splittotal"),
        "expected splitTotal error, got: {split_err}"
    );

    // 8. 17-field agent redaction hides all private fields
    let all_17: Vec<String> = [
        "name", "contactName", "contactEmail", "phone", "billingEmail", "domain", "taxId",
        "address", "stripeCustomerId", "notes", "amounts", "dates", "status", "tags", "plans",
        "method", "owner",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let view_out = crm
        .sync_stripe(CrmSyncRequest {
            source: "stripe".to_string(),
            private_fields: Some(all_17),
            for_agent_id: Some(aid.clone()),
            ..Default::default()
        })
        .expect("sync with 17 private fields for agent view");
    assert_eq!(view_out.private_fields.len(), 17);
    let dumped = serde_json::to_string(&view_out.agent_view.expect("agent_view present"))
        .expect("serialize agent_view");
    assert!(!dumped.contains("EU-998877"));
    assert!(!dumped.contains("cfo@nordic.test"));
    assert!(!dumped.contains("Confidential tier"));
    assert!(!dumped.contains("Nordic Labs"));

    // 9. 50 pending proposals cap enforced (51st fails)
    let cap_agent = env.create_agent("CrmCapAgent", None);
    for idx in 0..50 {
        let out = crm
            .propose_or_apply(CrmProposeRequest {
                agent_id: Some(cap_agent.clone()),
                operation: Some("customer-create".to_string()),
                payload: Some(json!({
                    "name": format!("Lead-{idx:02}"),
                    "currency": "USD",
                    "statusId": "lead"
                })),
                reason: Some("Batch proposal".to_string()),
                ..Default::default()
            })
            .expect("proposal within 50 cap");
        assert_eq!(out.status, "pending_approval");
    }
    let cap_err = crm
        .propose_or_apply(CrmProposeRequest {
            agent_id: Some(cap_agent),
            operation: Some("customer-create".to_string()),
            payload: Some(json!({ "name": "Lead-51", "currency": "USD", "statusId": "lead" })),
            reason: Some("Overflow proposal".to_string()),
            ..Default::default()
        })
        .expect_err("51st pending proposal must fail");
    assert!(matches!(cap_err, OpenBotRuntimeError::LimitExceeded(_)));
}

#[test]
fn test_ob_f12_static_sites_publish_replace_delete_limits_and_forbidden_paths() {
    let env = TestEnv::new();
    let sites = StaticSiteService::open(env.path()).expect("open StaticSiteService");

    // 1. Valid publish (30-day expiry)
    let site_dir = env.path().join("site_docs");
    fs::create_dir_all(&site_dir).expect("mkdir site_docs");
    fs::write(site_dir.join("index.html"), "<!doctype html><h1>Docs v1</h1>").expect("write index");
    fs::write(site_dir.join("styles.css"), "body { color: #111; }").expect("write css");
    fs::write(site_dir.join("app.js"), "console.log('v1');").expect("write js");

    let pub1 = sites
        .publish_site(StaticSitePublishRequest {
            slug: "docs-v1".to_string(),
            source_dir: site_dir.clone(),
            title: Some("Docs Portal".to_string()),
            description: Some("Static site v1".to_string()),
            replace: false,
            spa_fallback: true,
            allowed_roots: Some(vec![env.path().to_path_buf()]),
        })
        .expect("publish valid static site");
    assert_eq!(pub1.status, "active");
    assert_eq!(pub1.file_count, 3);
    assert_eq!(pub1.expires_in_days, 30);
    assert!(!pub1.replaced);
    assert!(!pub1.deleted);

    // 2. Replace preserves site_id and url
    fs::write(site_dir.join("index.html"), "<!doctype html><h1>Docs v2</h1>").expect("update index");
    let pub2 = sites
        .publish_site(StaticSitePublishRequest {
            slug: "docs-v1".to_string(),
            source_dir: site_dir,
            title: Some("Docs Portal v2".to_string()),
            description: None,
            replace: true,
            spa_fallback: true,
            allowed_roots: Some(vec![env.path().to_path_buf()]),
        })
        .expect("replace existing static site");
    assert_eq!(pub2.site_id, pub1.site_id);
    assert_eq!(pub2.url, pub1.url);
    assert!(pub2.replaced);
    assert_eq!(pub2.expires_in_days, 30);

    // 3. Delete marks deleted
    let del = sites
        .delete_site(StaticSiteDeleteRequest {
            slug: Some("docs-v1".to_string()),
            site_id: Some(pub1.site_id.clone()),
        })
        .expect("delete static site");
    assert!(del.deleted);
    assert_eq!(del.status, "deleted");
    assert_eq!(del.site_id, pub1.site_id);
    assert!(sites.list_sites().expect("list sites").iter().any(|s| s.site_id == pub1.site_id && s.deleted));

    // 4. Reject 21 files
    let dir_21 = env.path().join("site_21_files");
    fs::create_dir_all(&dir_21).expect("mkdir site_21_files");
    fs::write(dir_21.join("index.html"), "<h1>index</h1>").expect("write index");
    for i in 0..20 {
        fs::write(dir_21.join(format!("page_{i}.html")), "<p>ok</p>").expect("write page");
    }
    let _ = sites
        .publish_site(StaticSitePublishRequest {
            slug: "too-many-files".to_string(),
            source_dir: dir_21,
            title: None,
            description: None,
            replace: false,
            spa_fallback: false,
            allowed_roots: Some(vec![env.path().to_path_buf()]),
        })
        .expect_err("reject 21 files");

    // 5. Reject > 1 MiB single file
    let dir_big_file = env.path().join("site_big_file");
    fs::create_dir_all(&dir_big_file).expect("mkdir site_big_file");
    fs::write(dir_big_file.join("index.html"), "<h1>index</h1>").expect("write index");
    fs::write(dir_big_file.join("huge.txt"), vec![b'A'; 1_048_577]).expect("write >1 MiB file");
    let _ = sites
        .publish_site(StaticSitePublishRequest {
            slug: "too-big-file".to_string(),
            source_dir: dir_big_file,
            title: None,
            description: None,
            replace: false,
            spa_fallback: false,
            allowed_roots: Some(vec![env.path().to_path_buf()]),
        })
        .expect_err("reject >1 MiB single file");

    // 6. Reject > 2 MiB total (3 x 800 KiB files, each < 1 MiB)
    let dir_big_total = env.path().join("site_big_total");
    fs::create_dir_all(&dir_big_total).expect("mkdir site_big_total");
    fs::write(dir_big_total.join("index.html"), "<h1>index</h1>").expect("write index");
    for i in 0..3 {
        fs::write(dir_big_total.join(format!("part_{i}.txt")), vec![b'B'; 800_000]).expect("write part");
    }
    let _ = sites
        .publish_site(StaticSitePublishRequest {
            slug: "too-big-total".to_string(),
            source_dir: dir_big_total,
            title: None,
            description: None,
            replace: false,
            spa_fallback: false,
            allowed_roots: Some(vec![env.path().to_path_buf()]),
        })
        .expect_err("reject >2 MiB total");

    // 7. Reject missing index.html
    let dir_no_index = env.path().join("site_no_index");
    fs::create_dir_all(&dir_no_index).expect("mkdir site_no_index");
    fs::write(dir_no_index.join("about.html"), "<h1>No index</h1>").expect("write about");
    let _ = sites
        .publish_site(StaticSitePublishRequest {
            slug: "no-index".to_string(),
            source_dir: dir_no_index,
            title: None,
            description: None,
            replace: false,
            spa_fallback: false,
            allowed_roots: Some(vec![env.path().to_path_buf()]),
        })
        .expect_err("reject missing index.html");

    // 8. Reject all 15 forbidden paths
    assert!(StaticSiteService::validate_relative_path("index.html").is_ok());
    assert!(StaticSiteService::validate_relative_path("assets/app.js").is_ok());
    let forbidden_paths = [
        "../escape.html",
        ".git/config",
        ".DS_Store",
        ".env",
        "node_modules/a/index.js",
        "server/index.js",
        "api/x.js",
        "credentials.json",
        "private_key.pem",
        "my_secret.txt",
        "worker.js",
        "app.js.map",
        "bundle.zip",
        "backup.tar",
        "data.gz",
    ];
    for bad in forbidden_paths {
        assert!(
            StaticSiteService::validate_relative_path(bad).is_err(),
            "expected forbidden static site path to be rejected: {bad}"
        );
    }
}

#[test]
fn test_ob_f12_marketplace_tabs_source_filter_and_fishing_fallback() {
    let env = TestEnv::new();
    let market = MarketplaceService::open(env.path()).expect("open MarketplaceService");

    for tab in ["apps", "agents", "skills"] {
        let out = market
            .search(MarketplaceSearchRequest {
                tab: tab.to_string(),
                ..Default::default()
            })
            .expect("search valid marketplace tab");
        assert_eq!(out.tab, tab);
        assert_eq!(out.tabs, vec!["apps", "agents", "skills"]);
        assert!(!out.items.is_empty());
        assert!(!out.empty_state);
    }

    let _ = market
        .search(MarketplaceSearchRequest {
            tab: "invalid-tab".to_string(),
            ..Default::default()
        })
        .expect_err("reject invalid marketplace tab");

    let verified = market
        .search(MarketplaceSearchRequest {
            tab: "skills".to_string(),
            source: Some("verified".to_string()),
            ..Default::default()
        })
        .expect("verified source filter");
    assert!(!verified.items.is_empty());
    assert!(verified.items.iter().all(|i| i.verified));

    let public = market
        .search(MarketplaceSearchRequest {
            tab: "skills".to_string(),
            source: Some("public".to_string()),
            ..Default::default()
        })
        .expect("public source filter");
    assert!(public.items.iter().all(|i| !i.verified));

    let first_slug = verified.items[0].slug.clone();
    let installed = market
        .install_item(MarketplaceTab::Skills, &first_slug)
        .expect("install verified skill");
    assert_eq!(installed.slug, first_slug);

    let zero_anim = market
        .search(MarketplaceSearchRequest {
            tab: "skills".to_string(),
            query: Some("no-match-xyz-9999".to_string()),
            ..Default::default()
        })
        .expect("zero-match search");
    assert!(zero_anim.items.is_empty());
    assert!(zero_anim.empty_state);
    assert_eq!(zero_anim.alpha_fallback, "fishing");
    assert_eq!(zero_anim.plush_asset, "fishing.webm");

    let zero_reduced = market
        .search(MarketplaceSearchRequest {
            tab: "skills".to_string(),
            query: Some("no-match-xyz-9999".to_string()),
            reduced_motion: true,
            ..Default::default()
        })
        .expect("reduced motion empty state");
    assert_eq!(zero_reduced.plush_asset, "fishing-still.webp");

    let zero_no_alpha = market
        .search(MarketplaceSearchRequest {
            tab: "skills".to_string(),
            query: Some("no-match-xyz-9999".to_string()),
            alpha_unsupported: true,
            ..Default::default()
        })
        .expect("alpha unsupported empty state");
    assert_eq!(zero_no_alpha.plush_asset, "fishing-still.webp");
}

#[test]
fn test_ob_f12_html_sandbox_openbot_page_rpc_backdrops_and_credit_media() {
    let env = TestEnv::new();
    let html_svc = HtmlAndCreditMediaService::open(env.path()).expect("open HtmlAndCreditMediaService");

    // 1. Preview mode (no openbotPage, CSP enforced)
    let preview = html_svc
        .render_or_generate(HtmlRenderRequest {
            mode: Some("preview".to_string()),
            html: Some("<section>Preview Card</section>".to_string()),
            preview_width: Some(360),
            ..Default::default()
        })
        .expect("render preview mode");
    assert_eq!(preview.mode, "preview");
    assert!(preview.sandboxed);
    assert!(preview.csp_enforced);
    assert_eq!(preview.csp_header, "sandbox allow-scripts allow-forms");
    assert!(!preview.openbot_page);
    assert_eq!(preview.preview_width, Some(360));

    // 2. Page mode + openbotPage JSON-RPC
    let page = html_svc
        .render_or_generate(HtmlRenderRequest {
            mode: Some("page".to_string()),
            html: Some("<main id='app'>Interactive Page</main>".to_string()),
            frame_height: Some(320),
            rpc_message: Some(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "ui/message",
                "params": { "text": "Ship widget" }
            })),
            ..Default::default()
        })
        .expect("render page mode");
    assert_eq!(page.mode, "page");
    assert!(page.openbot_page);
    assert_eq!(page.csp_header, "sandbox allow-scripts allow-forms");
    assert_eq!(page.frame_height, Some(320));
    assert!(page.rpc_result.is_some());

    // Direct openbotPage RPC checks
    let rpc_msg = html_svc.evaluate_openbot_page_rpc(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "ui/message",
        "params": { "text": "Hello from widget" }
    }));
    assert!(!rpc_msg.refused);
    assert_eq!(rpc_msg.rpc_type, "message");

    let rpc_https = html_svc.evaluate_openbot_page_rpc(&json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "ui/open-link",
        "params": { "url": "https://openbot.dev/docs" }
    }));
    assert!(!rpc_https.refused);
    assert_eq!(rpc_https.rpc_type, "open-link");

    for bad_url in ["javascript:alert(1)", "file:///etc/passwd"] {
        let refused = html_svc.evaluate_openbot_page_rpc(&json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "ui/open-link",
            "params": { "url": bad_url }
        }));
        assert!(refused.refused, "must refuse unsafe url {bad_url}");
        assert_eq!(refused.rpc_type, "refused");
    }

    let refused_ctx = html_svc.evaluate_openbot_page_rpc(&json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "ui/update-model-context",
        "params": { "context": "C".repeat(2_001) }
    }));
    assert!(refused_ctx.refused);
    assert_eq!(refused_ctx.rpc_type, "refused");

    // 3. HTML size (>512,000) & height (79 / 2001) rejections
    let _ = html_svc
        .render_or_generate(HtmlRenderRequest {
            mode: Some("page".to_string()),
            html: Some("H".repeat(512_001)),
            ..Default::default()
        })
        .expect_err("reject 512,001-char HTML");
    for bad_h in [79u32, 2_001u32] {
        let _ = html_svc
            .render_or_generate(HtmlRenderRequest {
                mode: Some("page".to_string()),
                html: Some("<div>ok</div>".to_string()),
                frame_height: Some(bad_h),
                ..Default::default()
            })
            .expect_err("reject out-of-range frame height");
    }

    // 4. Backdrop mode + <meta name="openbot-bubbles"> injection + aliases
    let bd = html_svc
        .render_or_generate(HtmlRenderRequest {
            mode: Some("backdrop".to_string()),
            backdrop: Some("pond".to_string()),
            html: Some("<html><head></head><body><canvas></canvas></body></html>".to_string()),
            bubbles_yours: Some("#3b93f0".to_string()),
            bubbles_agents: Some("#212121".to_string()),
            ..Default::default()
        })
        .expect("render backdrop mode");
    assert_eq!(bd.mode, "backdrop");
    assert!(!bd.openbot_page);
    assert!(bd.bubbles_meta.as_deref().unwrap_or("").contains("openbot-bubbles"));
    assert!(bd.rendered_html.as_deref().unwrap_or("").contains("openbot-bubbles"));

    assert_eq!(html_svc.validate_backdrop("pond").expect("pond"), "pond");
    assert_eq!(html_svc.validate_backdrop("forest").expect("forest alias"), "jellyfish");
    assert_eq!(html_svc.validate_backdrop("city").expect("city alias"), "sunset");
    assert_eq!(html_svc.validate_backdrop("snow").expect("snow alias"), "snowfall");
    assert_eq!(html_svc.validate_backdrop("desert").expect("desert alias"), "fireflies");
    assert_eq!(html_svc.validate_backdrop("coast").expect("coast"), "coast");
    assert!(html_svc.validate_backdrop("unknown-scene").is_err());

    // 5. 50-credit gate: generated (direct & approved), declined, short, unavailable
    let gen_direct = html_svc
        .render_or_generate(HtmlRenderRequest {
            generate_media: true,
            media_kind: Some("image".to_string()),
            media_model: Some("gpt-image-2.6".to_string()),
            media_prompt: Some("Pixel lighthouse".to_string()),
            cost_credits: Some(30),
            wallet_balance: Some(200),
            approval_threshold: Some(50),
            agents_credits_allowed: Some(true),
            ..Default::default()
        })
        .expect("generate image <= 50 credits");
    assert!(!gen_direct.approval_required);
    assert_eq!(gen_direct.outcome, Some(CreditMediaOutcome::Generated));
    assert_eq!(gen_direct.charged_credits, Some(30));
    assert_eq!(gen_direct.remaining_balance, Some(170));

    let gen_approved = html_svc
        .render_or_generate(HtmlRenderRequest {
            generate_media: true,
            media_kind: Some("video".to_string()),
            media_model: Some("seedance-2.5".to_string()),
            media_prompt: Some("Ocean waves".to_string()),
            cost_credits: Some(80),
            wallet_balance: Some(170),
            approval_threshold: Some(50),
            agents_credits_allowed: Some(true),
            approve_media: true,
            ..Default::default()
        })
        .expect("generate approved video > 50 credits");
    assert!(gen_approved.approval_required);
    assert_eq!(gen_approved.outcome, Some(CreditMediaOutcome::Generated));
    assert_eq!(gen_approved.charged_credits, Some(80));
    assert_eq!(gen_approved.remaining_balance, Some(90));

    let declined = html_svc
        .render_or_generate(HtmlRenderRequest {
            generate_media: true,
            media_kind: Some("video".to_string()),
            cost_credits: Some(75),
            wallet_balance: Some(200),
            approval_threshold: Some(50),
            agents_credits_allowed: Some(true),
            decline_media: true,
            ..Default::default()
        })
        .expect("declined video > 50 credits");
    assert_eq!(declined.outcome, Some(CreditMediaOutcome::Declined));
    assert_eq!(declined.charged_credits.unwrap_or(0), 0);
    assert_eq!(declined.remaining_balance, Some(200));

    let short = html_svc
        .render_or_generate(HtmlRenderRequest {
            generate_media: true,
            media_kind: Some("image".to_string()),
            cost_credits: Some(40),
            wallet_balance: Some(10),
            approval_threshold: Some(50),
            agents_credits_allowed: Some(true),
            ..Default::default()
        })
        .expect("short wallet balance");
    assert_eq!(short.outcome, Some(CreditMediaOutcome::Short));

    let unavailable = html_svc
        .render_or_generate(HtmlRenderRequest {
            generate_media: true,
            media_kind: Some("image".to_string()),
            cost_credits: Some(20),
            wallet_balance: Some(200),
            approval_threshold: Some(50),
            agents_credits_allowed: Some(false),
            ..Default::default()
        })
        .expect("agents credits disallowed -> unavailable");
    assert_eq!(unavailable.outcome, Some(CreditMediaOutcome::Unavailable));
}

// ============================================================================
// OB-F13: Voice Service (STT, TTS, 25 Live Voices, VoiceServerSettings)
// ============================================================================

#[test]
fn test_ob_f13_voice_stt_tts_live_voices_and_server_settings_redaction() {
    let env = TestEnv::new();
    let voice = VoiceService::open(env.db.clone(), env.path()).expect("open VoiceService");

    // 1. STT locale routing, bark-3.0-plus retired flag, >600s rejected
    let stt_zh = voice
        .eval(VoiceEvalRequest {
            mode: "stt".to_string(),
            locale: Some("zh-CN".to_string()),
            audio_seconds: Some(120),
            ..Default::default()
        })
        .expect("eval zh STT");
    assert!(stt_zh.valid);
    assert_eq!(stt_zh.model_id.as_deref(), Some("whisper-turbo"));

    let stt_en = voice
        .eval(VoiceEvalRequest {
            mode: "stt".to_string(),
            locale: Some("en-US".to_string()),
            audio_seconds: Some(600),
            ..Default::default()
        })
        .expect("eval en STT");
    assert!(stt_en.valid);
    assert_eq!(stt_en.model_id.as_deref(), Some("parakeet-tdt-0.6b-v3"));

    let stt_bark = voice
        .eval(VoiceEvalRequest {
            mode: "stt".to_string(),
            stt_model: Some("bark-3.0-plus".to_string()),
            ..Default::default()
        })
        .expect("eval retired bark-3.0-plus");
    assert_eq!(stt_bark.retired, Some(true));

    let stt_over = voice
        .eval(VoiceEvalRequest {
            mode: "stt".to_string(),
            audio_seconds: Some(601),
            ..Default::default()
        })
        .expect_err("reject >600s audio");
    assert!(matches!(stt_over, OpenBotRuntimeError::LimitExceeded(_)));

    // 2. TTS next_speech_speed cycle, channel lead voice_agent_id, >1000 chars rejected
    let lead_agent = env.create_agent("LeadVoice", None);
    let member_agent = env.create_agent("MemberVoice", None);
    let channel_id = env.create_channel("VoiceRoom", &[&lead_agent, &member_agent], Some(&lead_agent));

    let tts = voice
        .eval(VoiceEvalRequest {
            mode: "tts".to_string(),
            tts_provider: Some("supertonic-3".to_string()),
            tts_text: Some("Build verification succeeded.".to_string()),
            tts_speed: Some(1.0),
            tts_speed_steps: Some(5),
            channel_id: Some(channel_id),
            ..Default::default()
        })
        .expect("eval TTS");
    assert!(tts.valid);
    assert_eq!(tts.speed_cycle, Some(vec![1.25, 1.5, 2.0, 0.75, 1.0]));
    assert_eq!(tts.next_speed, Some(1.0));
    assert_eq!(tts.voice_agent_id.as_deref(), Some(lead_agent.as_str()));

    let tts_over = voice
        .eval(VoiceEvalRequest {
            mode: "tts".to_string(),
            tts_text: Some("T".repeat(1_001)),
            ..Default::default()
        })
        .expect_err("reject >1,000-char TTS text");
    assert!(matches!(tts_over, OpenBotRuntimeError::LimitExceeded(_)));

    // 3. All 25 Live Voices accepted & >65,536 SDP rejected
    assert_eq!(CODEX_LIVE_VOICES.len(), 25);
    let valid_sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=OpenBotLive\r\n";
    for live_voice_name in CODEX_LIVE_VOICES {
        let out = voice
            .eval(VoiceEvalRequest {
                mode: "live".to_string(),
                live_voice: Some(live_voice_name.to_string()),
                sdp: Some(valid_sdp.to_string()),
                agent_id: Some(lead_agent.clone()),
                ..Default::default()
            })
            .expect("accept all 25 Codex Live Voices");
        assert!(out.valid);
        assert_eq!(out.live_voice.as_deref(), Some(live_voice_name));
        assert_eq!(out.max_sdp_length, Some(65_536));
    }

    let session = voice
        .start_live_session(
            VoiceLiveStart {
                agent_id: lead_agent.clone(),
                sdp: valid_sdp.to_string(),
            },
            Some(CodexLiveVoice::Juniper),
        )
        .expect("start live voice session");
    assert!(!session.session_id.is_empty());
    assert!(voice.stop_live_session(&session.session_id).expect("stop live voice session"));

    let _ = voice
        .eval(VoiceEvalRequest {
            mode: "live".to_string(),
            live_voice: Some("juniper".to_string()),
            sdp: Some("S".repeat(65_537)),
            agent_id: Some(lead_agent),
            ..Default::default()
        })
        .expect_err("reject >65,536-byte SDP");

    // 4. VoiceServerSettings strips apiKey on serialization and reports hasApiKey: true
    let routes = VoiceServerRoutes {
        dictation: true,
        speech: true,
        transcription_model: Some("whisper-large-v3".to_string()),
        speech_model: Some("tts-1".to_string()),
        voice: Some("alloy".to_string()),
    };
    let saved_srv = voice
        .save_voice_server_settings(
            "https://voice.internal.test".to_string(),
            Some("xi-super-secret-voice-key-999".to_string()),
            routes,
        )
        .expect("save VoiceServerSettings");
    assert!(saved_srv.has_api_key);
    let fetched = voice
        .get_voice_server_settings()
        .expect("load VoiceServerSettings")
        .expect("stored VoiceServerSettings present");
    assert!(fetched.has_api_key);
    let srv_val = serde_json::to_value(&fetched).expect("serialize VoiceServerSettings");
    assert_eq!(srv_val.get("hasApiKey"), Some(&json!(true)));
    assert!(srv_val.get("apiKey").is_none());
    assert!(srv_val.get("api_key").is_none());
    assert!(!serde_json::to_string(&fetched).expect("json str").contains("xi-super-secret-voice-key-999"));
}

// ============================================================================
// OB-F13: External Bridges (WhatsApp & Discord ACLs)
// ============================================================================

#[test]
fn test_ob_f13_bridges_whatsapp_br_9th_digit_and_sender_decisions() {
    // 1. Brazilian 9th-digit bidirectional normalization
    let forms_13 = ExternalBridgeOrchestrator::whatsapp_number_forms("5511987654321");
    assert!(forms_13.contains(&"5511987654321".to_string()));
    assert!(forms_13.contains(&"551187654321".to_string()));
    let forms_12 = ExternalBridgeOrchestrator::whatsapp_number_forms("551187654321");
    assert!(forms_12.contains(&"5511987654321".to_string()));
    assert!(forms_12.contains(&"551187654321".to_string()));

    // 2. whatsapp_sender_decision (Allow / Save / Pair / Ignore)
    let rules_allow = WhatsAppSenderRules {
        own_number: false,
        contacts: vec!["551187654321".to_string(), "48500100200".to_string()],
        groups: Vec::new(),
        read_only: vec!["48500100200".to_string()],
    };
    assert_eq!(
        ExternalBridgeOrchestrator::whatsapp_sender_decision(
            &rules_allow,
            &WhatsAppSenderFacts {
                is_self_chat: false,
                is_group: false,
                from_me: false,
                chat_jid: "5511987654321@s.whatsapp.net".to_string(),
                contact_number: Some("5511987654321".to_string()),
                author_number: Some("5511987654321".to_string()),
            },
        ),
        WhatsAppDecision::Allow
    );
    assert_eq!(
        ExternalBridgeOrchestrator::whatsapp_sender_decision(
            &rules_allow,
            &WhatsAppSenderFacts {
                is_self_chat: false,
                is_group: false,
                from_me: false,
                chat_jid: "48500100200@s.whatsapp.net".to_string(),
                contact_number: Some("48500100200".to_string()),
                author_number: Some("48500100200".to_string()),
            },
        ),
        WhatsAppDecision::Save
    );
    assert_eq!(
        ExternalBridgeOrchestrator::whatsapp_sender_decision(
            &rules_allow,
            &WhatsAppSenderFacts {
                is_self_chat: false,
                is_group: false,
                from_me: false,
                chat_jid: "48600700800@s.whatsapp.net".to_string(),
                contact_number: Some("48600700800".to_string()),
                author_number: Some("48600700800".to_string()),
            },
        ),
        WhatsAppDecision::Pair
    );
    assert_eq!(
        ExternalBridgeOrchestrator::whatsapp_sender_decision(
            &WhatsAppSenderRules { own_number: true, ..Default::default() },
            &WhatsAppSenderFacts {
                is_self_chat: false,
                is_group: false,
                from_me: false,
                chat_jid: "48600700800@s.whatsapp.net".to_string(),
                contact_number: Some("48600700800".to_string()),
                author_number: Some("48600700800".to_string()),
            },
        ),
        WhatsAppDecision::Ignore
    );
    assert_eq!(
        ExternalBridgeOrchestrator::whatsapp_sender_decision(
            &rules_allow,
            &WhatsAppSenderFacts {
                is_self_chat: false,
                is_group: true,
                from_me: false,
                chat_jid: "99999@g.us".to_string(),
                contact_number: None,
                author_number: Some("48600700800".to_string()),
            },
        ),
        WhatsAppDecision::Ignore
    );
}

#[test]
fn test_ob_f13_bridges_whatsapp_pairing_3_cap_and_discord_roles() {
    let env = TestEnv::new();
    let bridges = ExternalBridgeOrchestrator::open(env.db.clone(), env.path()).expect("open bridges");

    // 1. WhatsApp max 3 pending pairing requests cap
    let mut codes = Vec::new();
    for idx in 1..=3 {
        let out = bridges
            .eval_bridge(BridgeEvalRequest {
                platform: "whatsapp".to_string(),
                workspace_id: Some("wa-pair-cap".to_string()),
                sender: Some(format!("4860070080{idx}")),
                text: Some("Pair me".to_string()),
                own_number: Some(false),
                allowed_senders: Some(Vec::new()),
                ..Default::default()
            })
            .expect("pairing request within 3 cap");
        assert_eq!(out.decision, "pair");
        assert_eq!(out.pairing_allowed, Some(true));
        assert_eq!(out.pending_requests_capped, Some(false));
        codes.push(out.pairing_code.expect("6-char pairing code"));
    }

    let capped = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "whatsapp".to_string(),
            workspace_id: Some("wa-pair-cap".to_string()),
            sender: Some("48600700804".to_string()),
            text: Some("4th pending request".to_string()),
            own_number: Some(false),
            allowed_senders: Some(Vec::new()),
            ..Default::default()
        })
        .expect("4th pairing request evaluated as capped");
    assert_eq!(capped.decision, "pair");
    assert_eq!(capped.pairing_allowed, Some(false));
    assert_eq!(capped.pending_requests_capped, Some(true));
    assert!(capped.pairing_code.is_none());

    assert!(
        bridges
            .answer_whatsapp_pairing("wa-pair-cap", &codes[0], true)
            .expect("approve first WhatsApp pairing request")
    );

    // 2. Discord: empty allowlist = allow all, matching/non-matching roles, <=50 cap, dup/invalid rejection
    let disc_open = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            channel_id: Some("chan-1".to_string()),
            text: Some("Open channel".to_string()),
            allowed_roles: Some(Vec::new()),
            author_roles: Some(vec!["999000".to_string()]),
            ..Default::default()
        })
        .expect("empty Discord allowlist allows all");
    assert!(disc_open.allowed);
    assert_eq!(disc_open.decision, "allow");

    let disc_allow = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            channel_id: Some("chan-1".to_string()),
            text: Some("Role ok".to_string()),
            allowed_roles: Some(vec![json!("777888")]),
            author_roles: Some(vec!["777888".to_string()]),
            ..Default::default()
        })
        .expect("matching Discord role allowed");
    assert!(disc_allow.allowed);
    assert_eq!(disc_allow.decision, "allow");

    let disc_deny = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            channel_id: Some("chan-1".to_string()),
            text: Some("Role denied".to_string()),
            allowed_roles: Some(vec![json!("777888")]),
            author_roles: Some(vec!["999000".to_string()]),
            ..Default::default()
        })
        .expect("non-matching Discord role ignored");
    assert!(!disc_deny.allowed);
    assert_eq!(disc_deny.decision, "ignore");

    let roles_50: Vec<Value> = (1..=50).map(|i| json!(format!("{}", 100_000 + i))).collect();
    let disc_50 = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            allowed_roles: Some(roles_50.clone()),
            author_roles: Some(vec!["100001".to_string()]),
            ..Default::default()
        })
        .expect("accept exactly 50 Discord roles");
    assert!(disc_50.allowed);

    let mut roles_51 = roles_50;
    roles_51.push(json!("100051"));
    let _ = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            allowed_roles: Some(roles_51),
            author_roles: Some(vec!["100001".to_string()]),
            ..Default::default()
        })
        .expect_err("reject 51 Discord roles (>50)");

    let _ = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            allowed_roles: Some(vec![json!("111222"), json!("111222")]),
            author_roles: Some(vec!["111222".to_string()]),
            ..Default::default()
        })
        .expect_err("reject duplicate Discord role IDs");

    let _ = bridges
        .eval_bridge(BridgeEvalRequest {
            platform: "discord".to_string(),
            workspace_id: Some("guild-1".to_string()),
            allowed_roles: Some(vec![json!("not-a-snowflake!!")]),
            author_roles: Some(vec!["111222".to_string()]),
            ..Default::default()
        })
        .expect_err("reject invalid Discord role ID");
}

struct SseFrame {
    id: Option<String>,
    event: Option<String>,
    data: String,
}

struct SseReader {
    resp: reqwest::Response,
    buf: String,
}

impl SseReader {
    fn new(resp: reqwest::Response) -> Self {
        Self {
            resp,
            buf: String::new(),
        }
    }

    async fn next_matching<F>(&mut self, timeout: Duration, mut pred: F) -> SseFrame
    where
        F: FnMut(&SseFrame) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            while let Some(pos) = self.buf.find("\n\n") {
                let raw_block = self.buf[..pos].to_string();
                self.buf.drain(..pos + 2);
                let mut id = None;
                let mut event = None;
                let mut data_lines = Vec::new();
                for line in raw_block.lines() {
                    if line.starts_with(':') || line.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        let val = v.strip_prefix(' ').unwrap_or(v);
                        match k {
                            "id" => id = Some(val.to_string()),
                            "event" => event = Some(val.to_string()),
                            "data" => data_lines.push(val.to_string()),
                            _ => {}
                        }
                    }
                }
                if !data_lines.is_empty() || event.is_some() || id.is_some() {
                    let frame = SseFrame {
                        id,
                        event,
                        data: data_lines.join("\n"),
                    };
                    if pred(&frame) {
                        return frame;
                    }
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for SSE event");
            let chunk = tokio::time::timeout(remaining, self.resp.chunk())
                .await
                .expect("SSE chunk timeout")
                .expect("SSE chunk IO")
                .expect("SSE stream ended before expected event");
            let normalized = String::from_utf8_lossy(&chunk).replace("\r\n", "\n");
            self.buf.push_str(&normalized);
        }
    }
}

// ============================================================================
// OB-F13: Live TeamApiServer (v1-v10, 309 Caps, CRUD, Cross-Handle SSE, WebRTC)
// ============================================================================

#[tokio::test]
async fn test_ob_f13_live_team_api_server_compatibility_crud_sse_replay_and_webrtc() {
    let env = TestEnv::new();
    let voice = VoiceService::open(env.db.clone(), env.path()).expect("open VoiceService");
    let handle = TeamApiServer::start(
        env.db.clone(),
        voice,
        TeamApiConfig {
            bind_port: 0,
            data_dir: env.path().to_path_buf(),
            require_auth_token: None,
        },
    )
    .await
    .expect("start TeamApiServer on 127.0.0.1:0");
    assert!(handle.addr().ip().is_loopback());

    let client = loopback_http_client();
    let base = handle.url();

    // 1. GET /v1/health -> 200
    let health_resp = client
        .get(format!("{base}/v1/health"))
        .send()
        .await
        .expect("GET /v1/health");
    assert_eq!(health_resp.status(), StatusCode::OK);
    let health_json: Value = health_resp.json().await.expect("parse health JSON");
    assert_eq!(health_json.get("ok"), Some(&json!(true)));
    assert_eq!(health_json.get("schema_version"), Some(&json!(69)));

    // 2. GET /v1/compatibility -> v1-v10, 309 capabilities, absent array
    let compat_resp = client
        .get(format!("{base}/v1/compatibility"))
        .header(TEAM_PROTOCOL_VERSION_HEADER, "10")
        .send()
        .await
        .expect("GET /v1/compatibility");
    assert_eq!(compat_resp.status(), StatusCode::OK);
    let compat_val: Value = compat_resp.json().await.expect("parse compatibility JSON");
    assert!(compat_val.get("absent").and_then(Value::as_array).is_some(), "v10 absent array present");
    let support: TeamProtocolSupport =
        serde_json::from_value(compat_val).expect("deserialize TeamProtocolSupport");
    assert_eq!(support.protocol, TeamProtocolRange { minimum: 1, maximum: 10 });
    let expanded = openbot_contracts::team_protocol::decode_team_protocol_support_expanded(&support)
        .expect("expand v10 capabilities");
    assert_eq!(expanded.capabilities.len(), 309);

    // Protocol evaluator & JoinedServerClient negotiation
    let eval = TeamApiServer::eval_protocol(TeamProtocolEvalRequest {
        client_version: Some(10),
        webrtc_transfer_id: Some("xfer-eval-1".to_string()),
        webrtc_offset: Some(1024),
        webrtc_payload_bytes: Some(256),
        ..Default::default()
    })
    .expect("eval TeamProtocol v10");
    assert_eq!(eval.negotiated_version, Some(10));
    assert_eq!(eval.total_capabilities, 309);
    assert_eq!(eval.expanded_capabilities.len(), 309);
    assert_eq!(eval.webrtc_roundtrip_ok, Some(true));

    // 3. OpenBot-Protocol-Version: 999 -> 426 Upgrade Required
    let resp_426 = client
        .get(format!("{base}/v1/agents"))
        .header(TEAM_PROTOCOL_VERSION_HEADER, "999")
        .send()
        .await
        .expect("GET /v1/agents with protocol 999");
    assert_eq!(resp_426.status(), StatusCode::UPGRADE_REQUIRED);

    // 4. POST/GET /v1/agents
    let post_ag = client
        .post(format!("{base}/v1/agents"))
        .json(&json!({ "name": "TeamRestAgent", "provider": "codex" }))
        .send()
        .await
        .expect("POST /v1/agents");
    assert_eq!(post_ag.status(), StatusCode::CREATED);
    let created_ag: Value = post_ag.json().await.expect("parse created agent");
    let agent_id = created_ag
        .get("id")
        .and_then(Value::as_str)
        .expect("agent id")
        .to_string();

    let list_ag: Vec<Value> = client
        .get(format!("{base}/v1/agents"))
        .send()
        .await
        .expect("GET /v1/agents")
        .json()
        .await
        .expect("parse agent list");
    assert!(list_ag.iter().any(|a| a.get("id").and_then(Value::as_str) == Some(agent_id.as_str())));

    // 5. POST/GET /v1/channels
    let post_ch = client
        .post(format!("{base}/v1/channels"))
        .json(&json!({
            "name": "TeamRestChannel",
            "members": [agent_id],
            "leadAgentId": agent_id
        }))
        .send()
        .await
        .expect("POST /v1/channels");
    assert_eq!(post_ch.status(), StatusCode::CREATED);
    let created_ch: Value = post_ch.json().await.expect("parse created channel");
    let channel_id = created_ch
        .get("id")
        .and_then(Value::as_str)
        .expect("channel id")
        .to_string();

    let list_ch: Vec<Value> = client
        .get(format!("{base}/v1/channels"))
        .send()
        .await
        .expect("GET /v1/channels")
        .json()
        .await
        .expect("parse channel list");
    assert!(list_ch.iter().any(|c| {
        c.get("channel")
            .and_then(|inner| inner.get("id"))
            .and_then(Value::as_str)
            == Some(channel_id.as_str())
    }));

    // 6. Live GET /v1/events SSE streaming a cross-handle SQLite ChannelStore message
    let sse1_resp = client
        .get(format!("{base}/v1/events"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .expect("connect SSE #1");
    assert_eq!(sse1_resp.status(), StatusCode::OK);
    let mut sse1 = SseReader::new(sse1_resp);

    let db2 = OpenBotDatabase::open_in_data_dir(env.path()).expect("open second SQLite handle");
    let ch_store2 = ChannelStore::new(db2);
    let ch_now = ch_store2.get(&channel_id).expect("load channel from handle #2");
    let msg1 = ChannelMessage {
        id: format!("msg-live-{}", uuid::Uuid::new_v4()),
        channel_id: channel_id.clone(),
        sequence: 1,
        author: ChannelMessageAuthor {
            kind: ChannelMessageAuthorKind::Member,
            id: "local".to_string(),
            name: "Tester".to_string(),
        },
        task_id: None,
        superseded: false,
        message: ConversationMessage::new_simple(
            "conv-live-1",
            ConversationMessageAuthor::User,
            "Cross-handle SSE live ping",
            Utc::now().to_rfc3339(),
            ConversationMessageStatus::Completed,
        ),
        deleted: None,
        sender_task_id: None,
    };
    let ch_after_1 = ch_store2
        .update(
            ch_now,
            ChannelUpdateChanges {
                messages: vec![msg1],
                ..Default::default()
            },
            None,
        )
        .expect("append live message via second handle");

    let live_frame = sse1
        .next_matching(Duration::from_secs(5), |f| {
            f.data.contains("Cross-handle SSE live ping")
        })
        .await;
    assert!(live_frame.event.is_some());
    let last_event_id = live_frame.id.expect("SSE event id present");

    // 7. Disconnect SSE #1, write missed event, reconnect with Last-Event-ID for replay
    drop(sse1);
    let msg2 = ChannelMessage {
        id: format!("msg-missed-{}", uuid::Uuid::new_v4()),
        channel_id: channel_id.clone(),
        sequence: 2,
        author: ChannelMessageAuthor {
            kind: ChannelMessageAuthorKind::Member,
            id: "local".to_string(),
            name: "Tester".to_string(),
        },
        task_id: None,
        superseded: false,
        message: ConversationMessage::new_simple(
            "conv-missed-2",
            ConversationMessageAuthor::User,
            "Missed event replayed via Last-Event-ID",
            Utc::now().to_rfc3339(),
            ConversationMessageStatus::Completed,
        ),
        deleted: None,
        sender_task_id: None,
    };
    let _ = ch_store2
        .update(
            ch_after_1,
            ChannelUpdateChanges {
                messages: vec![msg2],
                ..Default::default()
            },
            None,
        )
        .expect("append missed message while SSE disconnected");

    let sse2_resp = client
        .get(format!("{base}/v1/events"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header("Last-Event-ID", &last_event_id)
        .send()
        .await
        .expect("reconnect SSE #2 with Last-Event-ID");
    assert_eq!(sse2_resp.status(), StatusCode::OK);
    let mut sse2 = SseReader::new(sse2_resp);
    let replay_frame = sse2
        .next_matching(Duration::from_secs(5), |f| {
            f.data.contains("Missed event replayed via Last-Event-ID")
        })
        .await;
    assert_eq!(replay_frame.event.as_deref(), Some("channel.changed"));
    drop(sse2);

    // 8. WebRTC v2 chunk round-trip & >65,536 rejection + JoinedServerClient negotiation
    let joined = JoinedServerClient::open(env.path()).expect("open JoinedServerClient");
    assert_eq!(joined.negotiate_support(&support).expect("negotiate support"), 10);

    let payload = b"openbot-webrtc-v2-binary-chunk".to_vec();
    let frame = JoinedServerClient::encode_webrtc_file_chunk("xfer-01", 4096, &payload)
        .expect("encode WebRTC v2 chunk");
    let (dec_id, dec_offset, dec_bytes) =
        JoinedServerClient::decode_webrtc_file_chunk(&frame).expect("decode WebRTC v2 chunk");
    assert_eq!(dec_id, "xfer-01");
    assert_eq!(dec_offset, 4096);
    assert_eq!(dec_bytes, payload);

    let oversized = vec![0xABu8; 65_537];
    assert!(
        JoinedServerClient::encode_webrtc_file_chunk("xfer-big", 0, &oversized).is_err(),
        "WebRTC chunk > 65,536 bytes must be rejected"
    );

    handle.shutdown().await.expect("shutdown TeamApiServer");
}

#[tokio::test]
async fn test_ob_f13_openbot_serve_handle_and_crypto_helpers() {
    assert!(ct_eq(b"same-secret-token", b"same-secret-token"));
    assert!(!ct_eq(b"same-secret-token", b"diff-secret-token"));
    assert!(!ct_eq(b"short", b"longer-value"));

    let tok_a = generate_base64url_token_32();
    let tok_b = generate_base64url_token_32();
    assert_eq!(tok_a.len(), 43);
    assert_ne!(tok_a, tok_b);
    assert!(tok_a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));

    let env = TestEnv::new();
    let services = PlatformServices::open(env.db.clone(), env.path()).expect("open PlatformServices");
    let serve = OpenBotServeHandle::start(
        env.db.clone(),
        services,
        OpenBotServeConfig {
            data_dir: env.path().to_path_buf(),
            team_port: 0,
            automation_port: 0,
        },
    )
    .await
    .expect("start OpenBotServeHandle");

    let startup = serve.startup_json();
    assert_eq!(startup.get("team_api_url").and_then(Value::as_str), Some(serve.team_api.url().as_str()));
    assert_eq!(startup.get("automation_url").and_then(Value::as_str), Some(serve.automation.url().as_str()));
    assert_eq!(startup.get("automation_token").and_then(Value::as_str), Some(serve.automation.token()));

    let client = loopback_http_client();
    let health = client
        .get(format!("{}/v1/health", serve.team_api.url()))
        .send()
        .await
        .expect("GET /v1/health via OpenBotServeHandle");
    assert_eq!(health.status(), StatusCode::OK);

    serve.shutdown().await.expect("shutdown OpenBotServeHandle");
}

#[tokio::test]
async fn test_m4_host_runtime_platform_getters_tool_routing_and_serve() {
    use openbot_runtime::{OpenBotHostRuntime, ToolCallContext};

    let dir = TempDir::new().expect("create RAII tempfile::TempDir");
    let runtime = OpenBotHostRuntime::open(dir.path()).expect("open OpenBotHostRuntime");

    // 1. Verify all PlatformServices getters and typed extension attachment
    let platform = runtime.platform();
    assert!(std::sync::Arc::ptr_eq(platform, &runtime.extension::<PlatformServices>().expect("PlatformServices extension")));
    assert!(std::sync::Arc::ptr_eq(runtime.routines(), &platform.routines));
    assert!(std::sync::Arc::ptr_eq(runtime.workflows(), &platform.workflows));
    assert!(std::sync::Arc::ptr_eq(runtime.crm(), &platform.crm));
    assert!(std::sync::Arc::ptr_eq(runtime.static_sites(), &platform.static_sites));
    assert!(std::sync::Arc::ptr_eq(runtime.marketplace(), &platform.marketplace));
    assert!(std::sync::Arc::ptr_eq(runtime.html_media(), &platform.html_media));
    assert!(std::sync::Arc::ptr_eq(runtime.voice(), &platform.voice));
    assert!(std::sync::Arc::ptr_eq(runtime.bridges(), &platform.bridges));

    let agent = runtime
        .agents()
        .create_agent(CreateAgentInput {
            name: "PlatformBot".to_string(),
            description: "M4 host runtime integration test agent".to_string(),
            avatar_seed: "platformbot".to_string(),
            avatar_hue: None,
            avatar_color: None,
            initial_message: "Ready for routed platform tools.".to_string(),
            provider: Some(AgentProviderId::Codex),
            model: None,
            reasoning_effort: None,
            provider_account_id: None,
            operation_id: None,
        })
        .expect("create runtime agent");
    let ctx = ToolCallContext::for_agent(&agent.id);

    // 2. Enable CRM, seed Stripe fixture for agent, and route `crm_find` via tool_router
    runtime.crm().set_enabled(true).expect("enable CRM");
    let sync = runtime
        .crm()
        .sync_stripe(CrmSyncRequest {
            source: "stripe".to_string(),
            api_key: None,
            fixture_json: Some(json!({
                "customers": [{
                    "id": "cus_router_01",
                    "name": "Acme Labs",
                    "email": "founder@acme.test",
                    "currency": "usd",
                    "status": "customer"
                }],
                "plans": [],
                "payments": []
            })),
            private_fields: Some(vec!["contactEmail".to_string()]),
            for_agent_id: Some(agent.id.clone()),
        })
        .expect("sync CRM fixture");
    assert_eq!(sync.synced_customers, 1);

    let crm_out = runtime
        .tool_router()
        .call_tool(&ctx, "crm_find", r#"{"query":"Acme"}"#, false)
        .expect("route crm_find through tool_router");
    assert_eq!(crm_out.tool_name, "openbot.crm_find");
    let found_customers = crm_out
        .result
        .get("customers")
        .and_then(Value::as_array)
        .expect("crm_find returns customers array");
    assert_eq!(found_customers.len(), 1);
    assert_eq!(
        found_customers[0].get("name").and_then(Value::as_str),
        Some("Acme Labs")
    );

    // 3. Route `html_render` through tool_router
    let html_out = runtime
        .tool_router()
        .call_tool(
            &ctx,
            "html_render",
            r#"{"html":"<h1>Telemetry</h1>","title":"Dashboard","height":320}"#,
            false,
        )
        .expect("route html_render through tool_router");
    assert_eq!(html_out.tool_name, "openbot.html_render");
    assert_eq!(html_out.result.get("sandboxed").and_then(Value::as_bool), Some(true));
    assert_eq!(
        html_out.result.get("cspHeader").and_then(Value::as_str),
        Some("sandbox allow-scripts allow-forms")
    );
    assert_eq!(html_out.result.get("openbotPage").and_then(Value::as_bool), Some(true));
    assert_eq!(html_out.result.get("frameHeight").and_then(Value::as_u64), Some(320));

    // 4. Route `list_media_models` and `generate_image` through tool_router
    let models_out = runtime
        .tool_router()
        .call_tool(&ctx, "list_media_models", r#"{"kind":"image"}"#, false)
        .expect("route list_media_models");
    assert_eq!(models_out.tool_name, "openbot.list_media_models");
    assert!(
        models_out
            .result
            .get("models")
            .and_then(Value::as_array)
            .is_some_and(|arr| !arr.is_empty())
    );

    let img_out = runtime
        .tool_router()
        .call_tool(
            &ctx,
            "generate_image",
            r#"{"prompt":"Isometric plush bot workshop"}"#,
            false,
        )
        .expect("route generate_image through tool_router");
    assert_eq!(img_out.tool_name, "openbot.generate_image");
    assert_eq!(img_out.result.get("mediaKind").and_then(Value::as_str), Some("image"));
    assert_eq!(img_out.result.get("outcome").and_then(Value::as_str), Some("generated"));
    assert_eq!(img_out.result.get("chargedCredits").and_then(Value::as_i64), Some(2));

    // 5. Route `publish_site` and `list_sites` through tool_router
    let site_src = dir.path().join("routed_site");
    fs::create_dir_all(&site_src).expect("create site_src dir");
    fs::write(site_src.join("index.html"), "<!doctype html><h1>Hello Site</h1>")
        .expect("write index.html");
    let pub_args = json!({
        "sourcePath": site_src.to_string_lossy().to_string(),
        "title": "Routed Site",
        "description": "Published via tool_router"
    });
    let pub_out = runtime
        .tool_router()
        .call_tool_value(&ctx, "publish_site", pub_args)
        .expect("route publish_site through tool_router");
    assert_eq!(pub_out.tool_name, "openbot.publish_site");
    assert_eq!(pub_out.result.get("slug").and_then(Value::as_str), Some("routed-site"));

    let list_out = runtime
        .tool_router()
        .call_tool(&ctx, "list_sites", "{}", false)
        .expect("route list_sites through tool_router");
    assert_eq!(list_out.tool_name, "openbot.list_sites");
    assert_eq!(list_out.result.get("used").and_then(Value::as_u64), Some(1));
    assert_eq!(
        list_out
            .result
            .get("sites")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    // 6. Route `edit_workflow` + `list_workflows` + `suggest_marketplace_app`
    let wf_out = runtime
        .tool_router()
        .call_tool(
            &ctx,
            "edit_workflow",
            r#"{"title":"Router Flow","ops":[{"op":"add","ref":"t","kind":"trigger"},{"op":"add","ref":"o","kind":"output"},{"op":"connect","from":"t","to":"o"}]}"#,
            false,
        )
        .expect("route edit_workflow");
    assert_eq!(wf_out.tool_name, "openbot.edit_workflow");
    assert_eq!(wf_out.result.get("nodeCount").and_then(Value::as_u64), Some(2));

    let mkt_out = runtime
        .tool_router()
        .call_tool(&ctx, "suggest_marketplace_app", r#"{"query":"stripe"}"#, false)
        .expect("route suggest_marketplace_app");
    assert_eq!(mkt_out.tool_name, "openbot.suggest_marketplace_app");
    assert_eq!(mkt_out.result.get("alphaFallback").and_then(Value::as_str), Some("fishing"));

    // 7. Verify OpenBotHostRuntime::serve(0, 0) boots live Team API + Automation servers
    let serve = runtime.serve(0, 0).await.expect("boot runtime.serve(0, 0)");
    let client = loopback_http_client();
    let health = client
        .get(format!("{}/v1/health", serve.team_api.url()))
        .send()
        .await
        .expect("GET /v1/health via OpenBotHostRuntime::serve");
    assert_eq!(health.status(), StatusCode::OK);
    serve.shutdown().await.expect("shutdown runtime.serve");
}

#[tokio::test]
async fn test_m4_remediation_capabilities_protocol_headers_and_sse_lifecycle_events() {
    let env = TestEnv::new();
    let voice = VoiceService::open(env.db.clone(), env.path()).expect("open VoiceService");
    let handle = TeamApiServer::start(
        env.db.clone(),
        voice,
        TeamApiConfig {
            bind_port: 0,
            data_dir: env.path().to_path_buf(),
            require_auth_token: Some("secret-bootstrap-check".to_string()),
        },
    )
    .await
    .expect("start TeamApiServer with auth token");

    let client = loopback_http_client();
    let base = handle.url();
    let auth_header = "Bearer secret-bootstrap-check";

    // Blocker 1: GET /v1/capabilities (no auth required on bootstrap route) returns full contract payload & headers
    let caps_resp = client
        .get(format!("{base}/v1/capabilities"))
        .send()
        .await
        .expect("GET /v1/capabilities");
    assert_eq!(caps_resp.status(), StatusCode::OK);
    assert_eq!(
        caps_resp
            .headers()
            .get(TEAM_PROTOCOL_VERSION_HEADER.to_ascii_lowercase())
            .and_then(|v| v.to_str().ok()),
        Some("10")
    );
    assert!(
        caps_resp
            .headers()
            .get(TEAM_APP_VERSION_HEADER.to_ascii_lowercase())
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.is_empty())
    );
    assert!(
        caps_resp
            .headers()
            .get(TEAM_CAPABILITIES_HEADER.to_ascii_lowercase())
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains(TEAM_LONG_CAPABILITY_LISTS_CAPABILITY))
    );
    assert!(
        caps_resp
            .headers()
            .contains_key(TEAM_ABSENT_CAPABILITIES_HEADER.to_ascii_lowercase())
    );

    let caps_json: Value = caps_resp.json().await.expect("parse /v1/capabilities JSON");
    let expected_versions: Vec<Value> = (1..=10).map(|v| json!(format!("v{v}"))).collect();
    assert_eq!(
        caps_json.get("api_versions").and_then(Value::as_array),
        Some(&expected_versions)
    );
    let expected_providers: Vec<Value> = AgentProviderId::ALL
        .iter()
        .map(|p| json!(p.as_str()))
        .collect();
    assert_eq!(expected_providers.len(), 18);
    assert_eq!(
        caps_json.get("providers").and_then(Value::as_array),
        Some(&expected_providers)
    );
    let cap_list = caps_json
        .get("capabilities")
        .and_then(Value::as_array)
        .expect("capabilities array");
    assert_eq!(cap_list.len(), 256);
    assert!(cap_list.len() >= 220);
    assert!(
        cap_list
            .iter()
            .any(|c| c.as_str() == Some(TEAM_LONG_CAPABILITY_LISTS_CAPABILITY))
    );
    let unique_caps: std::collections::HashSet<&str> =
        cap_list.iter().filter_map(Value::as_str).collect();
    assert_eq!(unique_caps.len(), 256);
    assert_eq!(
        caps_json.get("absent").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );

    let support: TeamProtocolSupport =
        serde_json::from_value(caps_json).expect("deserialize capabilities as TeamProtocolSupport");
    let expanded = openbot_contracts::team_protocol::decode_team_protocol_support_expanded(&support)
        .expect("expand v10 capabilities");
    assert_eq!(expanded.capabilities.len(), 309);

    // Non-bootstrap route still requires auth token
    let unauth_agents = client
        .get(format!("{base}/v1/agents"))
        .send()
        .await
        .expect("unauthenticated GET /v1/agents");
    assert_eq!(unauth_agents.status(), StatusCode::UNAUTHORIZED);

    // Blocker 2: OpenBot-Protocol-Version validated on /v1/capabilities and /v1/compatibility, exempt on /v1/health
    for route in ["/v1/capabilities", "/v1/compatibility"] {
        let resp_0 = client
            .get(format!("{base}{route}"))
            .header(TEAM_PROTOCOL_VERSION_HEADER, "0")
            .send()
            .await
            .expect("GET with protocol 0");
        assert_eq!(resp_0.status(), StatusCode::UPGRADE_REQUIRED);
        let body_0: Value = resp_0.json().await.expect("parse 426 body for 0");
        assert_eq!(body_0.get("code"), Some(&json!("client_update_required")));

        let resp_99 = client
            .get(format!("{base}{route}"))
            .header(TEAM_PROTOCOL_VERSION_HEADER, "99")
            .send()
            .await
            .expect("GET with protocol 99");
        assert_eq!(resp_99.status(), StatusCode::UPGRADE_REQUIRED);
        let body_99: Value = resp_99.json().await.expect("parse 426 body for 99");
        assert_eq!(body_99.get("code"), Some(&json!("host_update_required")));

        let resp_bad = client
            .get(format!("{base}{route}"))
            .header(TEAM_PROTOCOL_VERSION_HEADER, "not-a-num")
            .send()
            .await
            .expect("GET with non-numeric protocol");
        assert_eq!(resp_bad.status(), StatusCode::BAD_REQUEST);
        let body_bad: Value = resp_bad.json().await.expect("parse 400 body");
        assert_eq!(body_bad.get("code"), Some(&json!("protocol_error")));

        let resp_ok = client
            .get(format!("{base}{route}"))
            .header(TEAM_PROTOCOL_VERSION_HEADER, "1")
            .send()
            .await
            .expect("GET with protocol 1");
        assert_eq!(resp_ok.status(), StatusCode::OK);
    }

    let health_with_99 = client
        .get(format!("{base}/v1/health"))
        .header(TEAM_PROTOCOL_VERSION_HEADER, "99")
        .send()
        .await
        .expect("GET /v1/health with protocol 99");
    assert_eq!(health_with_99.status(), StatusCode::OK);

    // Blocker 3: GET /v1/events SSE emits `ready` immediately, then `agent.created`, `channel.created`, `message.posted`, and replays on Last-Event-ID
    let sse_resp = client
        .get(format!("{base}/v1/events"))
        .header(reqwest::header::AUTHORIZATION, auth_header)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .expect("connect SSE");
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let mut sse = SseReader::new(sse_resp);

    let ready_frame = sse
        .next_matching(Duration::from_secs(3), |f| f.event.as_deref() == Some("ready"))
        .await;
    let ready_json: Value = serde_json::from_str(&ready_frame.data).expect("parse ready JSON");
    assert_eq!(ready_json.get("ok"), Some(&json!(true)));
    assert!(ready_json.get("cursor").and_then(Value::as_i64).is_some());

    // POST /v1/agents -> emits `agent.created`
    let ag_resp = client
        .post(format!("{base}/v1/agents"))
        .header(reqwest::header::AUTHORIZATION, auth_header)
        .json(&json!({ "name": "SseLifecycleBot", "provider": "codex" }))
        .send()
        .await
        .expect("POST /v1/agents");
    assert_eq!(ag_resp.status(), StatusCode::CREATED);
    let ag_val: Value = ag_resp.json().await.expect("parse created agent");
    let agent_id = ag_val
        .get("id")
        .and_then(Value::as_str)
        .expect("agent id")
        .to_string();

    let agent_ev = sse
        .next_matching(Duration::from_secs(5), |f| {
            f.event.as_deref() == Some("agent.created")
        })
        .await;
    assert!(agent_ev.id.is_some());

    // POST /v1/channels -> emits `channel.changed` AND `channel.created`
    let ch_resp = client
        .post(format!("{base}/v1/channels"))
        .header(reqwest::header::AUTHORIZATION, auth_header)
        .json(&json!({
            "name": "SseLifecycleRoom",
            "members": [agent_id],
            "leadAgentId": agent_id
        }))
        .send()
        .await
        .expect("POST /v1/channels");
    assert_eq!(ch_resp.status(), StatusCode::CREATED);
    let ch_val: Value = ch_resp.json().await.expect("parse created channel");
    let channel_id = ch_val
        .get("id")
        .and_then(Value::as_str)
        .expect("channel id")
        .to_string();

    let ch_created_ev = sse
        .next_matching(Duration::from_secs(5), |f| {
            f.event.as_deref() == Some("channel.created") && f.data.contains(&channel_id)
        })
        .await;
    let cursor_after_create = ch_created_ev.id.expect("channel.created id");

    // POST /v1/channels/{channel_id}/messages -> emits `channel.changed`, `channel.updated`, AND `message.posted`
    let msg_resp = client
        .post(format!("{base}/v1/channels/{channel_id}/messages"))
        .header(reqwest::header::AUTHORIZATION, auth_header)
        .json(&json!({ "text": "Live SSE message.posted check" }))
        .send()
        .await
        .expect("POST /v1/channels/{id}/messages");
    assert_eq!(msg_resp.status(), StatusCode::CREATED);

    let posted_ev = sse
        .next_matching(Duration::from_secs(5), |f| {
            f.event.as_deref() == Some("message.posted")
                && f.data.contains("Live SSE message.posted check")
        })
        .await;
    assert!(posted_ev.id.is_some());
    drop(sse);

    // Reconnect with Last-Event-ID = cursor_after_create -> replays `ready`, `channel.changed`, `channel.updated`, `message.posted`
    let replay_resp = client
        .get(format!("{base}/v1/events"))
        .header(reqwest::header::AUTHORIZATION, auth_header)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header("Last-Event-ID", &cursor_after_create)
        .send()
        .await
        .expect("reconnect SSE for replay");
    assert_eq!(replay_resp.status(), StatusCode::OK);
    let mut replay_sse = SseReader::new(replay_resp);

    let replay_ready = replay_sse
        .next_matching(Duration::from_secs(3), |f| f.event.as_deref() == Some("ready"))
        .await;
    let replay_ready_json: Value =
        serde_json::from_str(&replay_ready.data).expect("parse replay ready JSON");
    assert_eq!(
        replay_ready_json.get("cursor").and_then(Value::as_i64),
        cursor_after_create.parse::<i64>().ok()
    );

    let replay_raw = replay_sse
        .next_matching(Duration::from_secs(5), |f| {
            f.event.as_deref() == Some("channel.changed")
                && f.data.contains("Live SSE message.posted check")
        })
        .await;
    let replay_updated = replay_sse
        .next_matching(Duration::from_secs(5), |f| {
            f.event.as_deref() == Some("channel.updated")
                && f.data.contains("Live SSE message.posted check")
        })
        .await;
    let replay_posted = replay_sse
        .next_matching(Duration::from_secs(5), |f| {
            f.event.as_deref() == Some("message.posted")
                && f.data.contains("Live SSE message.posted check")
        })
        .await;
    assert_eq!(replay_raw.id, replay_updated.id);
    assert_eq!(replay_raw.id, replay_posted.id);
    drop(replay_sse);

    handle.shutdown().await.expect("shutdown TeamApiServer");
}

#[test]
fn test_m4_remediation_voice_speed_and_turn_pause_validation() {
    let env = TestEnv::new();
    let voice = VoiceService::open(env.db.clone(), env.path()).expect("open VoiceService");

    // 1. All supported SPEECH_SPEEDS accepted in eval_tts and save_speech_settings
    for speed in SPEECH_SPEEDS {
        let out = voice
            .eval(VoiceEvalRequest {
                mode: "tts".to_string(),
                tts_speed: Some(speed),
                ..Default::default()
            })
            .expect("accept supported TTS speed");
        assert!(out.valid);
        assert_eq!(out.speed, Some(speed));

        for pause_ms in VOICE_TURN_PAUSES {
            let saved = voice
                .save_speech_settings(SpeechSettings {
                    speed,
                    turn_pause_ms: pause_ms,
                    ..SpeechSettings::default()
                })
                .expect("accept supported speech speed and turn_pause_ms");
            assert_eq!(saved.speed, speed);
            assert_eq!(saved.turn_pause_ms, pause_ms);
        }
    }

    // 2. Unsupported tts_speed (3.5, 0.5, NaN) rejected with Validation in eval_tts
    for bad_speed in [3.5, 0.5, 0.0, -1.0, f64::NAN] {
        let err = voice
            .eval(VoiceEvalRequest {
                mode: "tts".to_string(),
                tts_speed: Some(bad_speed),
                ..Default::default()
            })
            .expect_err("reject unsupported TTS speed in eval_tts");
        assert!(
            matches!(err, OpenBotRuntimeError::Validation(ref msg) if msg.contains("Unsupported TTS playback speed")),
            "expected Validation error for speed {bad_speed}, got {err:?}"
        );
    }

    // 3. save_speech_settings rejects unsupported speed (3.5) and unsupported turn_pause_ms (999)
    let bad_speed_err = voice
        .save_speech_settings(SpeechSettings {
            speed: 3.5,
            ..SpeechSettings::default()
        })
        .expect_err("reject speed 3.5 in save_speech_settings");
    assert!(
        matches!(bad_speed_err, OpenBotRuntimeError::Validation(ref msg) if msg.contains("Unsupported TTS playback speed")),
        "expected Validation error for saved speed 3.5, got {bad_speed_err:?}"
    );

    let bad_pause_err = voice
        .save_speech_settings(SpeechSettings {
            turn_pause_ms: 999,
            ..SpeechSettings::default()
        })
        .expect_err("reject turn_pause_ms 999 in save_speech_settings");
    assert!(
        matches!(bad_pause_err, OpenBotRuntimeError::Validation(_)),
        "expected Validation error for turn_pause_ms 999, got {bad_pause_err:?}"
    );
}

