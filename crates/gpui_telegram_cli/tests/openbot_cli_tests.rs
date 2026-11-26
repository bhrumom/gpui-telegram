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

