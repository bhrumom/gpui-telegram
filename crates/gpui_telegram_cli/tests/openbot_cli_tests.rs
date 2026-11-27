//! Comprehensive unit & integration tests for the 9-subcommand `openbot` CLI (`OB-F20`).

use std::fs;

use gpui_telegram_cli::cli_parser::parse_args;
use gpui_telegram_cli::error::CliError;
use gpui_telegram_cli::openbot::serve_cmd::{parse_bind_port, start_serve_handles};
use gpui_telegram_cli::registry::{build_default_registry, default_cli_schema};
use gpui_telegram_cli::run_cli_with_args;
use serde_json::{Value, json};
use tempfile::TempDir;

const FORBIDDEN_PARSE_PATTERNS: [&str; 19] = [
    "unexpected argument",
    "unrecognized subcommand",
    "unknown option",
    "unknown argument",
    "invalid subcommand",
    "found argument '--",
    "required arguments were not provided",
    "unknown flag",
    "unknown global flag",
    "unknown subcommand",
    "unknown action",
    "unknown input field",
    "missing required argument",
    "requires an action subcommand",
    "requires a value",
    "does not take a value",
    "unexpected extra",
    "usage error:",
    "option '--",
];

fn args_with_dir(dir: &TempDir, parts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = vec!["openbot".to_string()];
    out.extend(parts.iter().map(|s| (*s).to_string()));
    out.push("--data-dir".to_string());
    out.push(dir.path().display().to_string());
    out.push("--json".to_string());
    out
}

async fn run_json(dir: &TempDir, parts: &[&str]) -> Value {
    let argv = args_with_dir(dir, parts);
    let stdout = run_cli_with_args(&argv)
        .await
        .unwrap_or_else(|e| panic!("CLI failed for {:?}: {e}", parts));
    let parsed: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("non-JSON stdout for {:?}: {e}\n{stdout}", parts));
    assert_eq!(parsed["success"], true, "expected success envelope: {parsed}");
    parsed["data"].clone()
}

fn assert_domain_error(err: &CliError, expected_exit: i32) {
    assert_eq!(err.exit_code(), expected_exit, "unexpected exit code for {err}");
    assert_ne!(err.code(), "usage_error", "domain error must not be usage_error: {err}");
    assert_ne!(
        err.code(),
        "unknown_command",
        "domain error must not be unknown_command: {err}"
    );
    let msg = err.to_string().to_ascii_lowercase();
    for pat in FORBIDDEN_PARSE_PATTERNS {
        assert!(
            !msg.contains(pat),
            "domain error leaked forbidden CLI parse pattern '{pat}': {msg}"
        );
    }
}

#[tokio::test]
async fn test_cli_parser_negative_values_empty_seed_and_syntax_vs_domain_codes() {
    let dir = TempDir::new().expect("tempdir");
    let registry = build_default_registry();
    let schema = default_cli_schema(&registry);

    // Negative values (--pointer -1.0,-1.0, --t -0.5) & empty --seed "" must parse cleanly
    let ok_args = args_with_dir(
        &dir,
        &[
            "avatar-eval",
            "--seed",
            "",
            "--pointer",
            "-1.0,-1.0",
            "--t",
            "-0.5",
            "--cluster-count",
            "8",
        ],
    );
    let parsed = parse_args(&ok_args, &registry, &schema, None).expect("parse negative args");
    assert_eq!(parsed.options.pretty, true);

    let avatar_data = run_json(
        &dir,
        &[
            "avatar-eval",
            "--seed",
            "",
            "--pointer",
            "-1.0,-1.0",
            "--cluster-count",
            "8",
        ],
    )
    .await;
    assert_eq!(avatar_data["profile"]["style"], "plush");
    assert_eq!(avatar_data["profile"]["shape"], "capsule");
    assert_eq!(avatar_data["profile"]["expression"], "excite");
    assert_eq!(avatar_data["profile"]["color"], "#e8483f");
    assert_eq!(avatar_data["cluster"]["shown_faces"], 3);
    assert_eq!(avatar_data["cluster"]["plus_pill"], "+5");

    // Genuine CLI syntax errors -> UnknownCommand / Usage (exit 2)
    let err_sub = run_cli_with_args(&args_with_dir(&dir, &["unknown-sub"]))
        .await
        .unwrap_err();
    assert_eq!(err_sub.exit_code(), 2);
    assert_eq!(err_sub.code(), "unknown_command");

    let err_flag = run_cli_with_args(&args_with_dir(&dir, &["ui-probe", "--unknown-flag"]))
        .await
        .unwrap_err();
    assert_eq!(err_flag.exit_code(), 2);
    assert_eq!(err_flag.code(), "usage_error");

    let err_missing = run_cli_with_args(&args_with_dir(&dir, &["chat", "send"]))
        .await
        .unwrap_err();
    assert_eq!(err_missing.exit_code(), 2);
    assert_eq!(err_missing.code(), "usage_error");

    // Domain validation errors -> Validation (exit 2) with ZERO parse-error strings
    let err_bad_json =
        run_cli_with_args(&args_with_dir(&dir, &["ui-probe", "--scenario", "{bad"]))
            .await
            .unwrap_err();
    assert_domain_error(&err_bad_json, 2);

    let err_unk_scene =
        run_cli_with_args(&args_with_dir(&dir, &["ui-probe", "--scenario", "no-such-probe"]))
            .await
            .unwrap_err();
    assert_domain_error(&err_unk_scene, 2);

    let err_goutte =
        run_cli_with_args(&args_with_dir(&dir, &["avatar-eval", "--shape", "goutte"]))
            .await
            .unwrap_err();
    assert_domain_error(&err_goutte, 2);

    assert_eq!(parse_bind_port("127.0.0.1:0").unwrap(), 0);
    assert!(parse_bind_port("0.0.0.0:8080").is_err());
}

#[tokio::test]
async fn test_theme_eval_and_all_15_ui_probe_scenarios() {
    let dir = TempDir::new().expect("tempdir");

    let theme = run_json(
        &dir,
        &[
            "theme-eval",
            "--mode",
            "dark",
            "--backdrop",
            "pond",
            "--reduced-motion",
            "--persist",
        ],
    )
    .await;
    assert_eq!(theme["mode"], "dark");
    assert!(theme["token_count"].as_u64().unwrap_or(0) >= 52);
    assert_eq!(theme["tokens"]["--openbot-bg-frame"], "#0d0d0d");
    assert_eq!(theme["tokens"]["--openbot-bg-canvas"], "#1a1a1a");
    assert_eq!(theme["transition"]["duration_ms"], 0);
    assert!(
        dir.path()
            .join("openbot-appearance-preference-v1.json")
            .is_file()
    );

    let bad_bd =
        run_cli_with_args(&args_with_dir(&dir, &["theme-eval", "--backdrop", "mars-colony"]))
            .await
            .unwrap_err();
    assert_domain_error(&bad_bd, 2);

    // Seed 1 agent + 1 channel so live fusion projection is verified in ui-probe
    let ag = run_json(&dir, &["agent", "create", "--name", "ProbeBot"]).await;
    let aid = ag["agent"]["id"].as_str().unwrap().to_string();
    let _ch = run_json(
        &dir,
        &["channel", "create", "--name", "ProbeRoom", "--members", &aid],
    )
    .await;

    let scenarios = [
        "openbot-shell",
        "conversation-virtualized",
        "composer-queue-sidepanel",
        "channel-conversation-recovery",
        "workflow-canvas-pages",
        "desktop-os-surfaces",
        "workspace-routing",
        "workspace-switch-theme-isolation",
        "telegram-bidirectional-fusion",
        "crew-pile-physics",
        "crew-pile-reduced-motion-and-broken-url",
        "avatar-horizon-cull",
        "avatar-mid-morph",
        "theme-hot-reload",
        "theme-sweep-reduced-motion",
    ];
    for sc in scenarios {
        let data = run_json(&dir, &["ui-probe", "--scenario", sc]).await;
        assert!(data.is_object(), "scenario {sc} must return JSON object");
    }

    let fusion = run_json(
        &dir,
        &[
            "ui-probe",
            "--scenario",
            r#"{"scenario":"telegram-bidirectional-fusion","sync_agents_to_telegram":true}"#,
        ],
    )
    .await;
    assert_eq!(fusion["bidirectional"]["projected_agent_peers"], 1);
    assert_eq!(fusion["bidirectional"]["projected_channel_peers"], 1);
    assert_eq!(fusion["telegram_baseline"]["nav_rail_px"], 52);
    assert_eq!(fusion["telegram_baseline"]["chat_sidebar_px"], 288);
    assert_eq!(fusion["telegram_baseline"]["inspector_px"], 320);
}

#[tokio::test]
async fn test_agent_chat_and_channel_cli_workflows() {
    let dir = TempDir::new().expect("tempdir");

    // 0 starter bots on fresh DB
    let list0 = run_json(&dir, &["agent", "list"]).await;
    assert_eq!(list0["agents"].as_array().unwrap().len(), 0);

    // Create + idempotent --command-id + defaults + zero temperature
    let c1 = run_json(
        &dir,
        &[
            "agent",
            "create",
            "--command-id",
            "cmd-cli-001",
            "--name",
            "Ada",
            "--title",
            "Architect",
            "--avatar-seed",
            "ada-seed",
            "--avatar-hue",
            "215",
            "--backup-models",
            r#"[{"provider":"claude","model":"claude-haiku-5-5"}]"#,
        ],
    )
    .await;
    let c2 = run_json(
        &dir,
        &[
            "agent",
            "create",
            "--command-id",
            "cmd-cli-001",
            "--name",
            "Ada",
        ],
    )
    .await;
    let aid = c1["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(aid, c2["agent"]["id"].as_str().unwrap());
    assert_eq!(c1["agent"]["provider"], "codex");
    assert_eq!(c1["agent"]["model"], "gpt-6-luna");
    assert_eq!(c1["agent"]["reasoningEffort"], "low");
    assert_eq!(c1["agent"]["allowAutomation"], false);
    assert!(c1["agent"].get("temperature").is_none());

    // ACP sets concurrent_turns = false
    let acp = run_json(
        &dir,
        &["agent", "create", "--name", "AcpBot", "--provider", "acp"],
    )
    .await;
    let aid2 = acp["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(acp["agent"]["concurrent_turns"], false);

    // generate-profile refuses muse
    let muse_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "agent",
            "generate-profile",
            "--prompt",
            "Helper bot",
            "--provider",
            "muse",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&muse_err, 2);

    // Export + Import round-trip into isolated second dir
    let bundle_path = dir.path().join("export.openbot");
    let bundle_str = bundle_path.display().to_string();
    let exp = run_json(&dir, &["agent", "export", "--ids", &aid, "--out", &bundle_str]).await;
    assert_eq!(exp["exported_count"], 1);
    let dest_dir = TempDir::new().expect("dest tempdir");
    let imp = run_json(&dest_dir, &["agent", "import", "--archive", &bundle_str]).await;
    assert_eq!(imp["agents"].as_array().unwrap().len(), 1);

    // Chat send, pin + human-pin protection, react + channel rejection, queue, DM, mailbox
    let sent = run_json(&dir, &["chat", "send", "--agent-id", &aid, "--text", "Hello"]).await;
    let msg_id = sent["messageId"].as_str().unwrap().to_string();

    let pinned = run_json(
        &dir,
        &[
            "chat",
            "pin",
            "--agent-id",
            &aid,
            "--message-id",
            &msg_id,
            "--pinned-by",
            "user",
        ],
    )
    .await;
    assert_eq!(pinned["pinned"], true);

    let unpin_agent = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "chat",
            "pin",
            "--agent-id",
            &aid,
            "--message-id",
            &msg_id,
            "--pinned-by",
            "agent",
            "--unpin",
        ],
    ))
    .await;
    assert!(unpin_agent.is_err());

    let dm = run_json(
        &dir,
        &[
            "chat",
            "dm-send",
            "--sender",
            "member-a",
            "--recipient",
            "member-b",
            "--client-message-id",
            "dm-001",
            "--text",
            "Direct hi",
        ],
    )
    .await;
    assert!(dm["thread_id"].as_str().unwrap().starts_with("dm-"));
    let dms = run_json(&dir, &["chat", "dm-list", "--member", "member-a"]).await;
    assert_eq!(dms["threads"].as_array().unwrap().len(), 1);

    let mb = run_json(
        &dir,
        &[
            "chat",
            "mailbox-send",
            "--sender-agent-id",
            &aid,
            "--recipient-agent-ids",
            &aid2,
            "--text",
            "Please review",
            "--expects-reply",
            "false",
        ],
    )
    .await;
    assert_eq!(mb["answerOptional"], true);

    // Channel create, rank-mentions, post, get, archive, restore
    let members = format!("{aid},{aid2}");
    let ch = run_json(
        &dir,
        &[
            "channel",
            "create",
            "--name",
            "CoreTeam",
            "--members",
            &members,
            "--lead",
            &aid,
        ],
    )
    .await;
    let cid = ch["channel"]["id"].as_str().unwrap().to_string();

    let ranked = run_json(
        &dir,
        &[
            "channel",
            "list",
            "--rank-mentions",
            "--channel-id",
            &cid,
            "--query",
            "ada",
        ],
    )
    .await;
    assert!(!ranked["ranked_members"].as_array().unwrap().is_empty());

    let posted = run_json(
        &dir,
        &[
            "channel",
            "post",
            "--channel-id",
            &cid,
            "--text",
            "Check build",
            "--router-mock",
            r#"{"agentIds":["m1"]}"#,
        ],
    )
    .await;
    assert!(posted["messageId"].as_str().is_some());

    let detail = run_json(&dir, &["channel", "get", "--channel-id", &cid]).await;
    assert!(!detail["messages"].as_array().unwrap().is_empty());
    assert!(detail["messages"][0]["text"].is_string());

    let _arch = run_json(&dir, &["channel", "archive", "--channel-id", &cid]).await;
    let post_archived = run_cli_with_args(&args_with_dir(
        &dir,
        &["channel", "post", "--channel-id", &cid, "--text", "Blocked"],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&post_archived, 2);
}

#[tokio::test]
async fn test_tools_platform_and_live_serve_cli_flows() {
    let dir = TempDir::new().expect("tempdir");

    let a1 = run_json(&dir, &["agent", "create", "--name", "ToolOwner"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a2 = run_json(&dir, &["agent", "create", "--name", "ToolPeer"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Tools catalog + search + describe + SQL owner authorizer + MCP reserved refusal
    let listed = run_json(&dir, &["tools", "list"]).await;
    assert_eq!(listed["directOpenbotCount"], 21);
    assert_eq!(listed["directBrowserCount"], 3);
    assert!(listed["catalogTotal"].as_u64().unwrap_or(0) >= 121);

    let created_tbl = run_json(
        &dir,
        &[
            "tools",
            "agent-data-exec",
            "--agent-id",
            &a1,
            "--mode",
            "write",
            "--sql",
            "CREATE TABLE kpi (k TEXT PRIMARY KEY, v INT)",
        ],
    )
    .await;
    assert_eq!(created_tbl["owner_agent_id"], a1);

    let drop_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "agent-data-exec",
            "--agent-id",
            &a2,
            "--mode",
            "write",
            "--sql",
            "DROP TABLE kpi",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&drop_err, 4);

    let mcp_reserved = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "mcp-register",
            "--name",
            "openbot",
            "--transport",
            "stdio",
            "--command",
            "echo",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&mcp_reserved, 2);

    // Memory put + CJK search
    let _mp = run_json(
        &dir,
        &[
            "tools",
            "memory-put",
            "--agent-id",
            &a1,
            "--text",
            "予約コードは ABC-99 です",
            "--inclusion",
            "searchable",
        ],
    )
    .await;
    let ms = run_json(
        &dir,
        &[
            "tools",
            "memory-search",
            "--agent-id",
            &a1,
            "--query",
            "予約コード",
        ],
    )
    .await;
    assert!(!ms["memories"].as_array().unwrap().is_empty());

    // Platform workflow-save (--spec-file) + workflow-run approval resume WITHOUT --run-id
    let wf_spec = json!({
        "title": "CliApprovalFlow",
        "nodes": [
            {"id": "n_trig", "kind": "trigger", "at": {"x": 0.0, "y": 0.0}, "fields": {"trigger": "manual"}},
            {"id": "n_appr", "kind": "approval", "at": {"x": 200.0, "y": 0.0}, "fields": {"question": "Approve?"}},
            {"id": "n_out", "kind": "output", "at": {"x": 400.0, "y": 0.0}, "fields": {"target": "notify", "message": "Shipped"}}
        ],
        "edges": [
            {"id": "e1", "from": "n_trig", "to": "n_appr", "type": "text"},
            {"id": "e2", "from": "n_appr", "to": "n_out", "type": "text"}
        ],
        "strokes": []
    });
    let wf_file = dir.path().join("wf.json");
    fs::write(&wf_file, serde_json::to_vec(&wf_spec).unwrap()).unwrap();
    let wf_file_str = wf_file.display().to_string();
    let saved_wf = run_json(&dir, &["platform", "workflow-save", "--spec-file", &wf_file_str]).await;
    let wf_id = saved_wf["workflow"]["id"].as_str().unwrap().to_string();

    let run1 = run_json(&dir, &["platform", "workflow-run", "--workflow-id", &wf_id]).await;
    assert_eq!(run1["workflow"]["status"], "awaiting_approval");

    let run2 = run_json(
        &dir,
        &[
            "platform",
            "workflow-run",
            "--workflow-id",
            &wf_id,
            "--approve-node",
            "n_appr",
        ],
    )
    .await;
    assert_eq!(run2["workflow"]["status"], "completed");

    // Platform routine-save + routine-run + site-publish --files + marketplace + voice-eval + bridge-sync
    let rt = run_json(
        &dir,
        &[
            "platform",
            "routine-save",
            "--agent-id",
            &a1,
            "--name",
            "CheckEvery3m",
            "--schedule",
            "every 180s",
            "--quiet-policy",
            "no_update",
        ],
    )
    .await;
    assert_eq!(rt["routine"]["interval_seconds"], 180);
    let rt_id = rt["routine"]["id"].as_str().unwrap().to_string();
    let rrun = run_json(
        &dir,
        &[
            "platform",
            "routine-run",
            "--routine-id",
            &rt_id,
            "--quiet-no-update",
        ],
    )
    .await;
    assert_eq!(rrun["routine"]["status"], "quiet_suppressed");

    let site = run_json(
        &dir,
        &[
            "platform",
            "site-publish",
            "--agent-id",
            &a1,
            "--slug",
            "cli-docs",
            "--files",
            r#"[{"path":"index.html","content":"<h1>Hi</h1>"}]"#,
        ],
    )
    .await;
    assert_eq!(site["site"]["status"], "active");
    let _del_site = run_json(&dir, &["platform", "site-delete", "--slug", "cli-docs"]).await;

    let mkt = run_json(&dir, &["platform", "marketplace-search", "--tab", "apps"]).await;
    assert!(!mkt["marketplace"]["items"].as_array().unwrap().is_empty());

    let stt = run_json(
        &dir,
        &[
            "platform",
            "voice-eval",
            "--mode",
            "stt",
            "--locale",
            "pl-PL",
            "--audio-rate-hz",
            "16000",
        ],
    )
    .await;
    assert_eq!(stt["voice"]["stt_providers"], 8);
    assert_eq!(stt["voice"]["selected_stt_model"], "parakeet-tdt-0.6b-v3");

    let bad_rate = run_cli_with_args(&args_with_dir(
        &dir,
        &["platform", "voice-eval", "--mode", "stt", "--audio-rate-hz", "44100"],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&bad_rate, 2);

    let br = run_json(
        &dir,
        &[
            "platform",
            "bridge-sync",
            "--platform",
            "telegram",
            "--direction",
            "inbound",
            "--agent-id",
            &a1,
            "--payload",
            r#"{"chat_id":"-100909","text":"Hello Telegram"}"#,
        ],
    )
    .await;
    assert_eq!(br["bridge"]["decision"], "allow");
    assert_eq!(br["bridge"]["delivered"], true);

    // Live serve startup JSON + /v1/health + /v1/capabilities + --team-token 401/200
    let servers = start_serve_handles(dir.path(), 0, 0, Some("secret-team-tok".to_string()))
        .await
        .expect("start serve handles");
    let team_url = servers.startup_json["team_api_url"]
        .as_str()
        .expect("team_api_url")
        .to_string();
    assert!(servers.startup_json["automation_url"].as_str().is_some());
    assert!(!servers.startup_json["automation_token"].as_str().unwrap().is_empty());

    let client = reqwest::Client::new();
    let health: Value = client
        .get(format!("{team_url}/v1/health"))
        .send()
        .await
        .expect("GET /v1/health")
        .json()
        .await
        .expect("health json");
    assert_eq!(health["schema_version"], 69);

    let caps_resp = client
        .get(format!("{team_url}/v1/capabilities"))
        .header("OpenBot-Protocol-Version", "10")
        .header("Authorization", "Bearer secret-team-tok")
        .send()
        .await
        .expect("GET /v1/capabilities");
    assert_eq!(caps_resp.status().as_u16(), 200);
    let caps: Value = caps_resp.json().await.expect("caps json");
    assert_eq!(caps["api_versions"].as_array().unwrap().len(), 10);
    assert_eq!(caps["providers"].as_array().unwrap().len(), 18);
    assert!(caps["capabilities"].as_array().unwrap().len() >= 220);

    let unauth = client
        .get(format!("{team_url}/v1/agents"))
        .send()
        .await
        .expect("GET /v1/agents without token");
    assert_eq!(unauth.status().as_u16(), 401);

    let authed = client
        .get(format!("{team_url}/v1/agents"))
        .header("Authorization", "Bearer secret-team-tok")
        .send()
        .await
        .expect("GET /v1/agents with token");
    assert_eq!(authed.status().as_u16(), 200);

    servers.shutdown().await.expect("clean serve shutdown");
}

#[tokio::test]
async fn test_channel_stop_before_cancel_lifecycle() {
    let dir = TempDir::new().expect("tempdir");

    let a1 = run_json(&dir, &["agent", "create", "--name", "StopLead"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a2 = run_json(&dir, &["agent", "create", "--name", "StopPeer"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let members = format!("{a1},{a2}");
    let ch = run_json(
        &dir,
        &[
            "channel",
            "create",
            "--name",
            "StopRoom",
            "--members",
            &members,
            "--lead",
            &a1,
        ],
    )
    .await;
    let cid = ch["channel"]["id"].as_str().unwrap().to_string();

    let posted = run_json(
        &dir,
        &[
            "channel",
            "post",
            "--channel-id",
            &cid,
            "--text",
            "Investigate active stop lifecycle",
            "--router-mock",
            r#"{"agentIds":["m1"]}"#,
            "--hold-running",
        ],
    )
    .await;
    let tid = posted["tasks"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(posted["tasks"][0]["state"], "running");

    // User cancel on an active (non-stopped) task must be refused per upstream channel-service.ts
    let cancel_before_stop = run_cli_with_args(&args_with_dir(
        &dir,
        &["channel", "cancel", "--channel-id", &cid, "--task-id", &tid],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&cancel_before_stop, 2);
    let err_lower = cancel_before_stop.to_string().to_ascii_lowercase();
    assert!(err_lower.contains("stopped") && err_lower.contains("paused"));

    // Stopping the active task transitions it to paused
    let stopped = run_json(
        &dir,
        &["channel", "stop", "--channel-id", &cid, "--task-id", &tid],
    )
    .await;
    assert_eq!(stopped["task"]["id"], tid);
    assert_eq!(stopped["task"]["state"], "paused");

    // Cancelling after stop transitions the paused task to cancelled
    let cancelled = run_json(
        &dir,
        &["channel", "cancel", "--channel-id", &cid, "--task-id", &tid],
    )
    .await;
    assert_eq!(cancelled["accepted"], true);
    assert_eq!(cancelled["task"]["id"], tid);
    assert_eq!(cancelled["task"]["state"], "cancelled");
}

#[tokio::test]
async fn test_agent_data_exec_authorizer_pragma_attach_and_mode_mismatch_diagnostics() {
    let dir = TempDir::new().expect("tempdir");

    let a1 = run_json(&dir, &["agent", "create", "--name", "SqlOwner"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a2 = run_json(&dir, &["agent", "create", "--name", "SqlPeer"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let created = run_json(
        &dir,
        &[
            "tools",
            "agent-data-exec",
            "--agent-id",
            &a1,
            "--mode",
            "write",
            "--sql",
            "CREATE TABLE orders (id INT PRIMARY KEY, item TEXT)",
        ],
    )
    .await;
    assert_eq!(created["owner_agent_id"], a1);

    // SELECT in write mode -> exit 2 (validation_error) with read mode guidance
    let sel_write_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "agent-data-exec",
            "--agent-id",
            &a1,
            "--mode",
            "write",
            "--sql",
            "SELECT * FROM orders",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&sel_write_err, 2);
    assert!(sel_write_err.to_string().to_ascii_lowercase().contains("read mode"));

    // INSERT in read mode -> exit 2 (validation_error) with read-only / write mode guidance
    let ins_read_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "agent-data-exec",
            "--agent-id",
            &a1,
            "--mode",
            "read",
            "--sql",
            "INSERT INTO orders VALUES (1, 'x')",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&ins_read_err, 2);
    let ins_lower = ins_read_err.to_string().to_ascii_lowercase();
    assert!(ins_lower.contains("read-only") && ins_lower.contains("write mode"));

    // PRAGMA & ATTACH -> exit 4 (forbidden) with 'not allowed'
    for sql in [
        "PRAGMA writable_schema = ON",
        "ATTACH DATABASE ':memory:' AS ext",
    ] {
        let err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "agent-data-exec",
                "--agent-id",
                &a1,
                "--mode",
                "write",
                "--sql",
                sql,
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&err, 4);
        assert!(err.to_string().to_ascii_lowercase().contains("not allowed"));
    }

    // Non-owner DROP / ALTER -> exit 4 with authorizer + not table owner
    for sql in [
        "DROP TABLE orders",
        "ALTER TABLE orders ADD COLUMN z TEXT",
    ] {
        let err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "agent-data-exec",
                "--agent-id",
                &a2,
                "--mode",
                "write",
                "--sql",
                sql,
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&err, 4);
        let lower = err.to_string().to_ascii_lowercase();
        assert!(lower.contains("authorizer") && lower.contains("not table owner"));
    }

    // Metadata DELETE -> exit 4 with authorizer + protected + metadata (and no 'no such table')
    let meta_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "agent-data-exec",
            "--agent-id",
            &a1,
            "--mode",
            "write",
            "--sql",
            "DELETE FROM _openbot_tables",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&meta_err, 4);
    let meta_lower = meta_err.to_string().to_ascii_lowercase();
    assert!(
        meta_lower.contains("authorizer")
            && meta_lower.contains("protected")
            && meta_lower.contains("metadata")
            && !meta_lower.contains("no such table")
    );
}

#[tokio::test]
async fn test_tools_fill_card_expiry_flags_mutual_exclusion_and_card_preservation() {
    let dir = TempDir::new().expect("tempdir");

    let aid = run_json(&dir, &["agent", "create", "--name", "CheckoutBot"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let ctx = "Exact regression test context exceeding one hundred characters for link-payment one-time card fill-card expiry mutual exclusion check.";
    let link = run_json(
        &dir,
        &[
            "tools",
            "link-payment",
            "--agent-id",
            &aid,
            "--amount",
            "1500",
            "--currency",
            "USD",
            "--merchant-name",
            "Acme",
            "--merchant-url",
            "https://checkout.example/pay",
            "--context",
            ctx,
            "--approve",
        ],
    )
    .await;
    let card_id = link["cardId"].as_str().unwrap().to_string();

    let browser = run_json(
        &dir,
        &[
            "tools",
            "browser-run",
            "--agent-id",
            &aid,
            "--url",
            "https://checkout.example/pay",
            "--steps",
            r#"[{"action":"snapshot"}]"#,
        ],
    )
    .await;
    let tab_id = browser["tabId"].as_str().unwrap().to_string();

    // 1. Blank --expiry must fail validation (exit 2) without consuming card
    let blank_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id,
            "--card-id",
            &card_id,
            "--expiry",
            "   ",
            "--approve",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&blank_err, 2);

    // 2. Combining --expiry with --exp-month/--exp-year must exit 2 WITHOUT consuming the Link card
    let conflict_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id,
            "--card-id",
            &card_id,
            "--expiry",
            "12/28",
            "--exp-month",
            "12",
            "--exp-year",
            "28",
            "--approve",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&conflict_err, 2);
    assert!(
        conflict_err
            .to_string()
            .contains("use expiry, or expMonth and expYear, not both"),
        "unexpected conflict error message: {conflict_err}"
    );

    // 3. Subsequent valid fill-card --expiry 12/28 --approve must succeed (card still unconsumed) and include "expiry"
    let filled = run_json(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id,
            "--card-id",
            &card_id,
            "--expiry",
            "12/28",
            "--approve",
        ],
    )
    .await;
    assert_eq!(filled["filledCard"], true);
    assert_eq!(filled["screenshotsBlocked"], true);
    let fields: Vec<&str> = filled["filledFields"]
        .as_array()
        .expect("filledFields array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        fields.contains(&"expiry"),
        "expected 'expiry' in filledFields, got {fields:?}"
    );
    assert!(
        !fields.contains(&"expMonth") && !fields.contains(&"expYear"),
        "unexpected split expiry fields alongside expiry: {fields:?}"
    );

    // 4. Separate one-time Link card with --exp-month / --exp-year populates expMonth & expYear
    let link2 = run_json(
        &dir,
        &[
            "tools",
            "link-payment",
            "--agent-id",
            &aid,
            "--amount",
            "2200",
            "--currency",
            "USD",
            "--merchant-name",
            "Acme",
            "--merchant-url",
            "https://checkout.example/pay2",
            "--context",
            ctx,
            "--approve",
        ],
    )
    .await;
    let card_id_2 = link2["cardId"].as_str().unwrap().to_string();
    let browser2 = run_json(
        &dir,
        &[
            "tools",
            "browser-run",
            "--agent-id",
            &aid,
            "--url",
            "https://checkout.example/pay2",
            "--steps",
            r#"[{"action":"snapshot"}]"#,
        ],
    )
    .await;
    let tab_id_2 = browser2["tabId"].as_str().unwrap().to_string();

    let filled_split = run_json(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id_2,
            "--card-id",
            &card_id_2,
            "--exp-month",
            "12",
            "--exp-year",
            "28",
            "--approve",
        ],
    )
    .await;
    assert_eq!(filled_split["filledCard"], true);
    let split_fields: Vec<&str> = filled_split["filledFields"]
        .as_array()
        .expect("filledFields array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        split_fields.contains(&"expMonth") && split_fields.contains(&"expYear"),
        "expected expMonth & expYear in filledFields, got {split_fields:?}"
    );
}

#[tokio::test]
async fn test_tools_fill_card_partial_split_bad_expiry_duplicate_targets_and_dm_strict_ids() {
    let dir = TempDir::new().expect("tempdir");

    let aid = run_json(&dir, &["agent", "create", "--name", "StrictPayBot"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let ctx = "Exact iteration-4 regression test context exceeding one hundred characters for one-time Link virtual card preservation under partial split expiry, malformed expiry, and duplicate targets.";
    let link = run_json(
        &dir,
        &[
            "tools",
            "link-payment",
            "--agent-id",
            &aid,
            "--amount",
            "1500",
            "--currency",
            "USD",
            "--merchant-name",
            "Acme",
            "--merchant-url",
            "https://checkout.example/pay",
            "--context",
            ctx,
            "--approve",
        ],
    )
    .await;
    let card_id = link["cardId"].as_str().unwrap().to_string();
    let browser = run_json(
        &dir,
        &[
            "tools",
            "browser-run",
            "--agent-id",
            &aid,
            "--url",
            "https://checkout.example/pay",
            "--steps",
            r#"[{"action":"snapshot"}]"#,
        ],
    )
    .await;
    let tab_id = browser["tabId"].as_str().unwrap().to_string();

    // 1. Partial split expiry via CLI flags, --targets JSON, and openbot_browser.fill_card tool call -> ALL exit 2, card preserved
    let partial_cli_cases: Vec<Vec<&str>> = vec![
        vec!["--exp-month", "12"],
        vec!["--exp-year", "28"],
        vec!["--targets", r#"{"expMonth":{"kind":"css","selector":"input#m"}}"#],
        vec!["--targets", r#"{"expYear":{"kind":"css","selector":"input#y"}}"#],
    ];
    for extra in partial_cli_cases {
        let mut argv = vec![
            "tools", "fill-card", "--agent-id", &aid, "--tab-id", &tab_id, "--card-id", &card_id,
        ];
        argv.extend(extra.iter().copied());
        argv.push("--approve");
        let err = run_cli_with_args(&args_with_dir(&dir, &argv))
            .await
            .unwrap_err();
        assert_domain_error(&err, 2);
        assert!(
            err.to_string().contains("provide both expMonth and expYear together"),
            "unexpected partial split error for {extra:?}: {err}"
        );
    }

    let tool_call_args = serde_json::json!({
        "tabId": tab_id,
        "cardId": card_id,
        "approved": true,
        "targets": { "expMonth": { "kind": "css", "selector": "input#m" } }
    })
    .to_string();
    let tool_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "call",
            "--agent-id",
            &aid,
            "--name",
            "openbot_browser.fill_card",
            "--arguments",
            &tool_call_args,
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&tool_err, 2);

    // 2. Out-of-range / malformed CLI expiry values -> ALL exit 2, card preserved
    let bad_expiry_cases: Vec<Vec<&str>> = vec![
        vec!["--exp-month", "13", "--exp-year", "28"],
        vec!["--exp-month", "0", "--exp-year", "28"],
        vec!["--exp-month", "-1", "--exp-year", "28"],
        vec!["--exp-month", "+12", "--exp-year", "28"],
        vec!["--exp-month", "abc", "--exp-year", "xyz"],
        vec!["--exp-month", "12", "--exp-year", "1999"],
        vec!["--exp-month", "12", "--exp-year", "2100"],
        vec!["--exp-month", "12", "--exp-year", "202"],
        vec!["--exp-month", "12", "--exp-year", "-28"],
        vec!["--expiry", "13/28"],
        vec!["--expiry", "00/28"],
        vec!["--expiry", "0/28"],
        vec!["--expiry", "-1/28"],
        vec!["--expiry", "not-a-date"],
        vec!["--expiry", "12/1999"],
        vec!["--expiry", "12/28/01"],
        vec!["--expiry", "12/abc"],
    ];
    for extra in bad_expiry_cases {
        let mut argv = vec![
            "tools", "fill-card", "--agent-id", &aid, "--tab-id", &tab_id, "--card-id", &card_id,
        ];
        argv.extend(extra.iter().copied());
        argv.push("--approve");
        let err = run_cli_with_args(&args_with_dir(&dir, &argv))
            .await
            .unwrap_err();
        assert_domain_error(&err, 2);
    }

    // 3. Duplicate target selectors (explicit-explicit, explicit-default, Ref-Css DOM collision, empty selector) -> ALL exit 2, card preserved
    let dup_target_cases = [
        r#"{"number":{"kind":"css","selector":"input#dup"},"expiry":{"kind":"css","selector":"input#dup"}}"#,
        r#"{"expiry":{"kind":"css","selector":"input[autocomplete=cc-number]"}}"#,
        r#"{"number":{"kind":"ref","ref":"e2"},"expiry":{"kind":"css","selector":"input[autocomplete=cc-number]"}}"#,
        r#"{"number":{"kind":"css","selector":"   "}}"#,
    ];
    for targets_json in dup_target_cases {
        let err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "fill-card",
                "--agent-id",
                &aid,
                "--tab-id",
                &tab_id,
                "--card-id",
                &card_id,
                "--targets",
                targets_json,
                "--approve",
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&err, 2);
    }

    // 4. Same one-time Link virtual card MUST still be unconsumed and succeed with valid expiry!
    let filled = run_json(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id,
            "--card-id",
            &card_id,
            "--expiry",
            "06/2028",
            "--approve",
        ],
    )
    .await;
    assert_eq!(filled["filledCard"], true);
    assert_eq!(filled["screenshotsBlocked"], true);

    // 5. chat dm-send strict identifier validation (padded, blank, and >128-char IDs -> exit 2)
    let id_200 = "x".repeat(200);
    let dm_bad_cases = [
        ("m1", " alice ", " bob "),
        (" cm-1 ", "alice", "bob"),
        ("   ", "alice", "bob"),
        (id_200.as_str(), "alice", "bob"),
        ("m2", id_200.as_str(), "bob"),
        ("m3", "alice", id_200.as_str()),
    ];
    for (cid, sender, recipient) in dm_bad_cases {
        let err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "chat",
                "dm-send",
                "--client-message-id",
                cid,
                "--sender",
                sender,
                "--recipient",
                recipient,
                "--text",
                "hello",
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&err, 2);
    }

    // Valid exact 128-char dm-send succeeds
    let id_128_a = "u".repeat(128);
    let id_128_b = "v".repeat(128);
    let id_128_m = "w".repeat(128);
    let dm_ok = run_json(
        &dir,
        &[
            "chat",
            "dm-send",
            "--client-message-id",
            &id_128_m,
            "--sender",
            &id_128_a,
            "--recipient",
            &id_128_b,
            "--text",
            "ok 128",
        ],
    )
    .await;
    assert_eq!(dm_ok["message_id"], id_128_m);
}

#[tokio::test]
async fn test_tools_fill_card_transactional_pre_resolution_vault_approval_and_point_collisions() {
    let dir = TempDir::new().expect("tempdir");

    let aid = run_json(&dir, &["agent", "create", "--name", "AtomicPayBot"]).await["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let ctx = "Exact iteration-5 regression test context exceeding one hundred characters verifying atomic pre-resolution before Link card consumption, Vault approval gating, padded CSS matching, and Point collisions.";
    let link = run_json(
        &dir,
        &[
            "tools",
            "link-payment",
            "--agent-id",
            &aid,
            "--amount",
            "4200",
            "--currency",
            "USD",
            "--merchant-name",
            "Merchant",
            "--merchant-url",
            "https://checkout.merchant.example/pay",
            "--context",
            ctx,
            "--approve",
        ],
    )
    .await;
    let link_card_id = link["cardId"].as_str().unwrap().to_string();

    let browser = run_json(
        &dir,
        &[
            "tools",
            "browser-run",
            "--agent-id",
            &aid,
            "--url",
            "https://checkout.merchant.example/pay",
            "--steps",
            r#"[{"action":"snapshot"}]"#,
        ],
    )
    .await;
    let tab_id = browser["tabId"].as_str().unwrap().to_string();
    let initial_el_count = browser["snapshot"]["elements"].as_array().unwrap().len();
    assert_eq!(initial_el_count, 6);

    // 1. D-I4-1: Unresolvable Ref, late-field nameOnCard Ref, stale revision, padded Ref,
    //    unmatched Role, unmatched Text -> ALL exit 2, tab untouched, Link card NOT consumed.
    let bad_resolution_targets = [
        r#"{"number":{"kind":"ref","ref":"e999"}}"#,
        r#"{"number":{"kind":"css","selector":"input[autocomplete=cc-number]"},"nameOnCard":{"kind":"ref","ref":"e999"}}"#,
        r#"{"number":{"kind":"ref","ref":"e1","revision":100}}"#,
        r#"{"number":{"kind":"ref","ref":" e1 "}}"#,
        r#"{"number":{"kind":"role","role":"textbox","name":"NoSuchCardField"}}"#,
        r#"{"number":{"kind":"text","text":"NoSuchCardText"}}"#,
    ];
    for targets_json in bad_resolution_targets {
        let err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "fill-card",
                "--agent-id",
                &aid,
                "--tab-id",
                &tab_id,
                "--card-id",
                &link_card_id,
                "--targets",
                targets_json,
                "--approve",
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&err, 2);
    }

    // Also via openbot_browser.fill_card tool call with early Css + late bad Ref -> exit 2,
    // no DOM upsert leak on real tab, Link card NOT consumed.
    let tool_call_args = json!({
        "tabId": tab_id,
        "cardId": link_card_id,
        "approved": true,
        "targets": {
            "number": { "kind": "css", "selector": "input#n" },
            "expiry": { "kind": "ref", "ref": "e999" }
        }
    })
    .to_string();
    let tool_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "call",
            "--agent-id",
            &aid,
            "--name",
            "openbot_browser.fill_card",
            "--arguments",
            &tool_call_args,
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&tool_err, 2);

    // 2. D-I4-2: Point collisions (Ref("e1") + Point, Point + Point) -> exit 2 with
    //    "Each card field needs its own target." AND Link card NOT consumed.
    let point_collision_targets = [
        r#"{"number":{"kind":"ref","ref":"e1"},"expiry":{"kind":"point","x":10.0,"y":20.0}}"#,
        r#"{"number":{"kind":"point","x":10.0,"y":20.0},"expiry":{"kind":"point","x":30.0,"y":40.0}}"#,
    ];
    for targets_json in point_collision_targets {
        let err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "fill-card",
                "--agent-id",
                &aid,
                "--tab-id",
                &tab_id,
                "--card-id",
                &link_card_id,
                "--targets",
                targets_json,
                "--approve",
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&err, 2);
        assert!(
            err.to_string().contains("Each card field needs its own target."),
            "expected duplicate-target error for {targets_json}, got: {err}"
        );
    }

    // Confirm tab stayed completely unlocked & unmutated (no e7 leak, filledCard=false)
    let check_snap = run_json(
        &dir,
        &[
            "tools",
            "browser-run",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id,
            "--steps",
            r#"[{"action":"snapshot"}]"#,
        ],
    )
    .await;
    assert_eq!(check_snap["snapshot"]["filledCard"], false);
    assert_eq!(check_snap["snapshot"]["screenshotsBlocked"], false);
    assert_eq!(
        check_snap["snapshot"]["elements"].as_array().unwrap().len(),
        initial_el_count
    );

    // 3. Same one-time Link virtual card MUST still be unconsumed; padded CSS selector
    //    " input[autocomplete=cc-number] " MUST match existing e2 without appending bogus e7!
    let filled = run_json(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id,
            "--card-id",
            &link_card_id,
            "--expiry",
            "12/28",
            "--targets",
            r#"{"number":{"kind":"css","selector":" input[autocomplete=cc-number] "}}"#,
            "--approve",
        ],
    )
    .await;
    assert_eq!(filled["filledCard"], true);
    assert_eq!(filled["screenshotsBlocked"], true);
    let snap_elements = filled["snapshot"]["elements"].as_array().unwrap();
    assert_eq!(
        snap_elements.len(),
        initial_el_count,
        "padded CSS selector must match existing e2 instead of appending duplicate e7: {snap_elements:?}"
    );
    let e2 = snap_elements
        .iter()
        .find(|el| el["ref"] == "e2")
        .expect("e2 present");
    assert_eq!(e2["selector"], "input[autocomplete=cc-number]");
    assert_eq!(e2["value"], "[redacted]");
    assert!(
        snap_elements.iter().all(|el| el["ref"] != "e7"),
        "bogus duplicate e7 must not be created"
    );

    // 4. D-I4-1 Saved Vault card approval-bypass guard:
    //    Failed fill-card --approve with bad Ref("e999") must NOT persist page_origin into fill_sites,
    //    so follow-up fill-card WITHOUT --approve MUST still exit 4 (approval_required)!
    let save_args = json!({
        "kind": "card",
        "label": "Saved Corp Card",
        "nameOnCard": "Ada Lovelace",
        "expMonth": 12,
        "expYear": 2029,
        "number": "4242424242424242",
        "securityCode": "321"
    })
    .to_string();
    let saved = run_json(
        &dir,
        &[
            "tools",
            "call",
            "--agent-id",
            &aid,
            "--name",
            "openbot_browser.save_vault_item",
            "--arguments",
            &save_args,
        ],
    )
    .await;
    let vault_card_id = saved["result"]["id"].as_str().unwrap().to_string();

    let browser2 = run_json(
        &dir,
        &[
            "tools",
            "browser-run",
            "--agent-id",
            &aid,
            "--url",
            "https://billing.savedcard.example/checkout",
            "--steps",
            r#"[{"action":"snapshot"}]"#,
        ],
    )
    .await;
    let tab_id_2 = browser2["tabId"].as_str().unwrap().to_string();

    let failed_saved = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id_2,
            "--card-id",
            &vault_card_id,
            "--targets",
            r#"{"number":{"kind":"ref","ref":"e999"}}"#,
            "--approve",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&failed_saved, 2);

    // Follow-up WITHOUT --approve MUST still require approval (exit 4, approval_required)!
    let needs_approval = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id_2,
            "--card-id",
            &vault_card_id,
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&needs_approval, 4);
    assert_eq!(needs_approval.code(), "approval_required");

    // Follow-up WITH --approve succeeds cleanly
    let filled_saved = run_json(
        &dir,
        &[
            "tools",
            "fill-card",
            "--agent-id",
            &aid,
            "--tab-id",
            &tab_id_2,
            "--card-id",
            &vault_card_id,
            "--approve",
        ],
    )
    .await;
    assert_eq!(filled_saved["filledCard"], true);
    assert_eq!(filled_saved["screenshotsBlocked"], true);
}

#[tokio::test]
async fn test_iter6_origin_isolation_webhook_secret_override_and_routine_owner_guards() {
    let dir = TempDir::new().expect("tempdir");
    let ag = run_json(&dir, &["agent", "create", "--name", "Iter6CliBot", "--provider", "codex"]).await;
    let aid = ag["agent"]["id"].as_str().unwrap().to_string();
    let ctx = "Purchase deterministic test license with strict origin isolation, PSL tenant separation, and HMAC enforcement audit verification!!";

    // 1. C1-D-I5-1: Cross-host IPv4, PSL private multi-tenant, and 2-level ccTLD isolation
    let cross_host_cases = [
        ("https://127.0.0.1/pay", "https://203.0.0.1/pay"),
        ("https://192.168.1.1/pay", "https://10.0.1.1/pay"),
        ("https://victim.github.io/pay", "https://attacker.github.io/pay"),
        ("https://victim.pages.dev/pay", "https://attacker.pages.dev/pay"),
        ("https://victim.vercel.app/pay", "https://attacker.vercel.app/pay"),
        ("https://victim.s3.amazonaws.com/pay", "https://attacker.s3.amazonaws.com/pay"),
        (
            "https://victim.s3.us-east-1.amazonaws.com/pay",
            "https://attacker.s3.us-east-1.amazonaws.com/pay",
        ),
        ("https://merchant.co.kr/pay", "https://attacker.co.kr/pay"),
        ("https://merchant.co.za/pay", "https://attacker.co.za/pay"),
        ("https://merchant.co.in/pay", "https://attacker.co.in/pay"),
    ];
    for (saved_url, evil_url) in cross_host_cases {
        let evil_tab_out = run_json(
            &dir,
            &[
                "tools",
                "browser-run",
                "--agent-id",
                &aid,
                "--url",
                evil_url,
                "--steps",
                r#"[{"action":"snapshot"}]"#,
            ],
        )
        .await;
        let evil_tab = evil_tab_out["tabId"].as_str().unwrap().to_string();

        // Link virtual payment card bound to saved_url MUST be forbidden (rc=4) on evil_url
        let issued = run_json(
            &dir,
            &[
                "tools",
                "link-payment",
                "--agent-id",
                &aid,
                "--amount",
                "1000",
                "--currency",
                "USD",
                "--merchant-name",
                "IsolatedShop",
                "--merchant-url",
                saved_url,
                "--context",
                ctx,
                "--approve",
            ],
        )
        .await;
        let card_id = issued["cardId"].as_str().unwrap().to_string();
        let card_err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "fill-card",
                "--agent-id",
                &aid,
                "--tab-id",
                &evil_tab,
                "--card-id",
                &card_id,
                "--approve",
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&card_err, 4);
        assert_eq!(card_err.code(), "forbidden");

        // Vault login saved for saved_url MUST NOT appear in list_logins and MUST fail vault-secret (rc=4) on evil_url
        let save_login_args = json!({
            "kind": "login",
            "title": format!("Login-{saved_url}"),
            "website": saved_url,
            "username": "admin",
            "password": "OnlyForSavedOrigin999!"
        })
        .to_string();
        let saved_login = run_json(
            &dir,
            &[
                "tools",
                "call",
                "--agent-id",
                &aid,
                "--name",
                "openbot_browser.save_vault_item",
                "--arguments",
                &save_login_args,
            ],
        )
        .await;
        let login_id = saved_login["result"]["id"].as_str().unwrap().to_string();

        let listed = run_json(
            &dir,
            &[
                "tools",
                "call",
                "--agent-id",
                &aid,
                "--name",
                "openbot_browser.list_logins",
                "--arguments",
                &json!({ "tabId": evil_tab }).to_string(),
            ],
        )
        .await;
        let leaked = listed["result"]["logins"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|l| l["id"] == login_id);
        assert!(
            !leaked,
            "list_logins leaked saved login for '{saved_url}' onto '{evil_url}'"
        );

        let sec_err = run_cli_with_args(&args_with_dir(
            &dir,
            &[
                "tools",
                "vault-secret",
                "--agent-id",
                &aid,
                "--tab-id",
                &evil_tab,
                "--method",
                "password",
                "--submission",
                "click",
                "--login-id",
                &login_id,
            ],
        ))
        .await
        .unwrap_err();
        assert_domain_error(&sec_err, 4);
        assert_eq!(sec_err.code(), "forbidden");
    }

    // Positive controls: same IPv4, same-org subdomain, and same-tenant github.io subdomain succeed
    for (bound_url, target_url) in [
        ("https://127.0.0.1/pay", "https://127.0.0.1/checkout"),
        ("https://checkout.shop.test/pay", "https://pay.shop.test/confirm"),
        (
            "https://checkout.victim.github.io/pay",
            "https://pay.victim.github.io/confirm",
        ),
    ] {
        let tab_out = run_json(
            &dir,
            &[
                "tools",
                "browser-run",
                "--agent-id",
                &aid,
                "--url",
                target_url,
                "--steps",
                r#"[{"action":"snapshot"}]"#,
            ],
        )
        .await;
        let tab_id = tab_out["tabId"].as_str().unwrap().to_string();
        let issued = run_json(
            &dir,
            &[
                "tools",
                "link-payment",
                "--agent-id",
                &aid,
                "--amount",
                "1000",
                "--currency",
                "USD",
                "--merchant-name",
                "ControlShop",
                "--merchant-url",
                bound_url,
                "--context",
                ctx,
                "--approve",
            ],
        )
        .await;
        let card_id = issued["cardId"].as_str().unwrap().to_string();
        let filled = run_json(
            &dir,
            &[
                "tools",
                "fill-card",
                "--agent-id",
                &aid,
                "--tab-id",
                &tab_id,
                "--card-id",
                &card_id,
                "--approve",
            ],
        )
        .await;
        assert_eq!(filled["filledCard"], true);
    }

    // 2. C2-D-I5-3: routine-save rejects blank/NBSP/padded/ghost agent & channel owners with rc=2 and 0 DB mutations
    for flag in ["--agent-id", "--channel-id"] {
        for bad_owner in ["   ", "\u{00a0}", " ghost-owner ", "nonexistent-owner-404"] {
            let err = run_cli_with_args(&args_with_dir(
                &dir,
                &[
                    "platform",
                    "routine-save",
                    flag,
                    bad_owner,
                    "--name",
                    "InvalidOwnerRoutine",
                    "--schedule",
                    "every 180s",
                ],
            ))
            .await
            .unwrap_err();
            assert_domain_error(&err, 2);
            assert_eq!(err.code(), "validation_error");
        }
    }

    let db = openbot_storage::OpenBotDatabase::open(dir.path().join("openbot.db")).expect("open db");
    let (agent_rt_cnt, chan_rt_cnt): (i64, i64) = db
        .with_conn(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM projection_agent_routines", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM projection_channel_routines", [], |r| r.get(0))?,
            ))
        })
        .expect("count routines");
    assert_eq!(agent_rt_cnt, 0);
    assert_eq!(chan_rt_cnt, 0);

    // 3. C2-D-I5-1: webhook-post --secret attacker_secret forged HMAC rejected with rc=3 and 0 runs
    let real_secret = "real_secret_999";
    let hook = run_json(
        &dir,
        &[
            "platform",
            "routine-save",
            "--agent-id",
            &aid,
            "--name",
            "CliWebhookGuard",
            "--schedule",
            "webhook",
            "--webhook-secret",
            real_secret,
        ],
    )
    .await;
    let rid = hook["routine"]["id"].as_str().unwrap().to_string();
    let ts = chrono::Utc::now().timestamp().to_string();
    let payload = r#"{"event":"pwn"}"#;
    let forged_sig =
        openbot_contracts::routines::sign_webhook_body("attacker_secret", &ts, "forge-1", payload.as_bytes());

    let forged_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "platform",
            "webhook-post",
            "--routine-id",
            &rid,
            "--payload",
            payload,
            "--timestamp",
            &ts,
            "--delivery-id",
            "forge-1",
            "--signature",
            &forged_sig,
            "--secret",
            "attacker_secret",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&forged_err, 3);
    assert_eq!(forged_err.code(), "unauthorized");

    let empty_secret_err = run_cli_with_args(&args_with_dir(
        &dir,
        &[
            "platform",
            "webhook-post",
            "--routine-id",
            &rid,
            "--payload",
            payload,
            "--timestamp",
            &ts,
            "--delivery-id",
            "forge-empty",
            "--signature",
            &openbot_contracts::routines::sign_webhook_body(
                real_secret,
                &ts,
                "forge-empty",
                payload.as_bytes(),
            ),
            "--secret",
            "",
        ],
    ))
    .await
    .unwrap_err();
    assert_domain_error(&empty_secret_err, 3);
    assert_eq!(empty_secret_err.code(), "unauthorized");

    let run_cnt_after_forge: i64 = db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM projection_routine_runs", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .expect("count runs");
    assert_eq!(run_cnt_after_forge, 0);

    // Authentic webhook without --secret succeeds using stored secret
    let authentic_sig =
        openbot_contracts::routines::sign_webhook_body(real_secret, &ts, "valid-1", payload.as_bytes());
    let ok_wh = run_json(
        &dir,
        &[
            "platform",
            "webhook-post",
            "--routine-id",
            &rid,
            "--payload",
            payload,
            "--timestamp",
            &ts,
            "--delivery-id",
            "valid-1",
            "--signature",
            &authentic_sig,
        ],
    )
    .await;
    assert_eq!(ok_wh["routine"]["webhookVerified"], true);
    assert_eq!(ok_wh["routine"]["status"], "started");
}

