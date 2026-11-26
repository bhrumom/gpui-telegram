//! `fabushi worktree` and `fabushi ide` (plus top-level aliases `browser`, `computer`,
//! `artifacts`, `plugin`, `serve`, `profile`) CLI handlers.

use std::collections::BTreeSet;
use std::path::Path;

use orca_core::orchestration_protocol::{
    ComputerUseAction, ComputerUsePolicy, ComputerUseRequest, ComputerUseTargetKind,
};
use orca_core::workspace_layout::{KeyChord, KeyModifiers};
use serde_json::{json, Map, Value};

use crate::cli_parser::GlobalOptions;
use crate::error::CliError;
use crate::orca_commands::{
    build_query_url, load_json_file_or_inline, orca_usage, pct_encode, sanitize_artifact_value,
    OrcaArgBag, OrcaCliHttp,
};

fn req_str<'a>(
    args: &'a OrcaArgBag,
    pos_idx: Option<usize>,
    flags: &[&str],
    label: &str,
) -> Result<&'a str, CliError> {
    if let Some(v) = args.first(flags).filter(|s| !s.trim().is_empty()) {
        return Ok(v);
    }
    if let Some(i) = pos_idx
        && let Some(v) = args.pos(i).filter(|s| !s.trim().is_empty())
    {
        return Ok(v);
    }
    Err(orca_usage(format!("Missing required argument {label}")))
}

pub(crate) async fn execute_worktree(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "parse-yaml" => {
            let yaml_text = if let Some(raw_yaml) = args.first(&["--yaml", "--content"]) {
                raw_yaml.to_string()
            } else if let Some(file_path) = args.first(&["--file"]) {
                std::fs::read_to_string(file_path).map_err(|e| {
                    orca_usage(format!("Failed reading orca.yaml file '{file_path}': {e}"))
                })?
            } else if let Some(pos) = args.pos(0) {
                if Path::new(pos).is_file() {
                    std::fs::read_to_string(pos).map_err(|e| {
                        orca_usage(format!("Failed reading orca.yaml file '{pos}': {e}"))
                    })?
                } else {
                    pos.to_string()
                }
            } else {
                return Err(orca_usage(
                    "Missing required <file> or --yaml argument for 'worktree parse-yaml'",
                ));
            };
            http.post("/api/v1/orca/ide/orca-yaml/parse", &json!({ "yaml": yaml_text }))
                .await
        }
        "include-copy" => {
            let source = req_str(args, Some(0), &["--source", "--source-repo-root"], "--source")?;
            let target = req_str(
                args,
                Some(1),
                &["--target", "--target-worktree-root"],
                "--target",
            )?;
            let rules = if let Some(raw_rules) = args.first(&["--rules"]) {
                if Path::new(raw_rules).is_file() {
                    std::fs::read_to_string(raw_rules).map_err(|e| {
                        orca_usage(format!("Failed reading rules file '{raw_rules}': {e}"))
                    })?
                } else {
                    raw_rules.to_string()
                }
            } else {
                let default_file = Path::new(source).join(".worktreeinclude");
                if default_file.is_file() {
                    std::fs::read_to_string(&default_file).unwrap_or_default()
                } else {
                    String::new()
                }
            };
            let mut body = json!({
                "source_repo_root": source,
                "target_worktree_root": target,
                "rules": rules,
            });
            if let Some(opts_raw) = args.first(&["--options-json"]) {
                body["options"] = load_json_file_or_inline(opts_raw, "options")?;
            } else {
                let mut opts_map = Map::new();
                if let Some(mb) = args.opt_u64(&["--max-bytes"])? {
                    opts_map.insert("max_bytes".into(), json!(mb));
                }
                if let Some(me) = args.opt_usize(&["--max-entries"])? {
                    opts_map.insert("max_entries".into(), json!(me));
                }
                if !opts_map.is_empty() {
                    body["options"] = Value::Object(opts_map);
                }
            }
            http.post("/api/v1/orca/ide/worktrees/include-copy", &body)
                .await
        }
        "list" => http.get("/api/v1/orca/ide/worktrees").await,
        "create" => {
            let repo_root = req_str(args, Some(0), &["--repo-root", "--repo"], "--repo-root")?;
            let wt_path = req_str(args, Some(1), &["--path", "--worktree-path"], "--path")?;
            let mut body = json!({
                "repo_root": repo_root,
                "worktree_path": wt_path,
            });
            if let Some(branch) = args.first(&["--branch", "--branch-name"]) {
                body["branch_name"] = json!(branch);
            }
            if let Some(title) = args.first(&["--title"]) {
                body["title"] = json!(title);
            }
            if let Some(parent) = args.first(&["--parent-worktree-id", "--parent"]) {
                body["parent_worktree_id"] = json!(parent);
            }
            http.post("/api/v1/orca/ide/worktrees", &body).await
        }
        "set" => {
            let id = req_str(args, Some(0), &["--id", "--worktree-id"], "<worktree_id>")?;
            let mut body = Map::new();
            if let Some(comment) = args.first(&["--comment", "--checkpoint-comment"]) {
                body.insert("checkpoint_comment".into(), json!(comment));
            }
            if let Some(status) = args.first(&["--status", "--workspace-status"]) {
                body.insert("workspace_status".into(), json!(status));
            }
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body.insert("now_ms".into(), json!(ms));
            }
            http.patch(
                &format!("/api/v1/orca/ide/worktrees/{}", pct_encode(id)),
                &Value::Object(body),
            )
            .await
        }
        "cleanup" | "remove" => {
            let id = req_str(args, Some(0), &["--id", "--worktree-id"], "<worktree_id>")?;
            let body = json!({
                "worktree_id": id,
                "unsaved_editor_tabs": args.all(&["--unsaved-tab"]),
                "force_discard_unsaved": args.has_bool(&["--force", "--force-discard-unsaved"]),
                "remove_directory_on_disk": args.has_bool(&["--remove-dir", "--remove-directory"]),
            });
            http.post("/api/v1/orca/ide/worktrees/cleanup", &body).await
        }
        "ports" => http.get("/api/v1/orca/ide/worktrees/ports").await,
        other => Err(CliError::UnknownCommand(format!(
            "Unknown 'worktree' subcommand '{other}'"
        ))),
    }
}

fn load_plugin_manifest_body(args: &OrcaArgBag) -> Result<Map<String, Value>, CliError> {
    let mut body = Map::new();
    if let Some(raw) = args.first(&["--manifest", "--file"]).or_else(|| args.pos(0)) {
        let parsed = load_json_file_or_inline(raw, "plugin manifest")?;
        if parsed.get("manifest").is_some() {
            if let Some(obj) = parsed.as_object() {
                for (k, v) in obj {
                    body.insert(k.clone(), v.clone());
                }
            }
        } else {
            body.insert("manifest".into(), parsed);
        }
    } else if let Some(id) = args.first(&["--id", "--plugin-id"]) {
        let manifest = json!({
            "id": id,
            "publisher": args.first(&["--publisher"]),
            "version": args.first(&["--version"]).unwrap_or("1.0.0"),
            "schema_version": args.opt_u32(&["--schema-version"])?.unwrap_or(2),
            "tier": args.first(&["--tier"]).unwrap_or("data"),
            "permissions": args.all(&["--permission"]),
            "tools": args.all(&["--tool"]),
            "hooks": [],
            "sha256": args.first(&["--sha256"]).unwrap_or("0000000000000000000000000000000000000000000000000000000000000000"),
        });
        body.insert("manifest".into(), manifest);
    } else {
        return Err(orca_usage(
            "Missing required <manifest.json> or --manifest argument",
        ));
    }
    if let Some(pkg) = args.first(&["--package-dir"]) {
        body.insert("package_dir".into(), json!(pkg));
    }
    if let Some(contribs) = args.first(&["--contributions-json"]) {
        body.insert(
            "contributions".into(),
            load_json_file_or_inline(contribs, "contributions")?,
        );
    }
    Ok(body)
}

pub(crate) async fn execute_ide(
    http: &OrcaCliHttp,
    opts: &GlobalOptions,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "diff-annotate" => {
            let file_path = req_str(args, None, &["--file", "--file-path"], "--file")?;
            let line_start = args
                .opt_u32(&["--line", "--line-start"])?
                .ok_or_else(|| orca_usage("Missing required --line <n>"))?;
            let body_text = req_str(args, Some(0), &["--body", "--comment", "--text"], "--body")?;
            let mut payload = json!({
                "file_path": file_path,
                "line_start": line_start,
                "body": body_text,
            });
            if let Some(le) = args.opt_u32(&["--line-end"])? {
                payload["line_end"] = json!(le);
            }
            if let Some(side) = args.first(&["--side"]) {
                payload["side"] = json!(side);
            }
            if let Some(aid) = args.first(&["--annotation-id", "--id"]) {
                payload["annotation_id"] = json!(aid);
            }
            http.post("/api/v1/orca/ide/diff-annotations", &payload)
                .await
        }
        "diff-list" => {
            let mut q = Vec::new();
            if let Some(fp) = args.first(&["--file", "--file-path"]).or_else(|| args.pos(0)) {
                q.push(("file_path", fp.to_string()));
            }
            if args.has_bool(&["--include-resolved"]) {
                q.push(("include_resolved", "true".to_string()));
            }
            if args.has_bool(&["--batch"]) {
                q.push(("batch", "true".to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/ide/diff-annotations", &q))
                .await
        }
        "diff-batch" => {
            let mut q = Vec::new();
            if let Some(fp) = args.first(&["--file", "--file-path"]).or_else(|| args.pos(0)) {
                q.push(("file_path", fp.to_string()));
            }
            http.get(&build_query_url(
                "/api/v1/orca/ide/diff-annotations/batch",
                &q,
            ))
            .await
        }
        "vault-search" => {
            let query = req_str(args, Some(0), &["--query", "-q", "--q"], "<query>")?;
            let mut q = vec![("q", query.to_string())];
            if let Some(ws) = args.first(&["--workspace", "--workspace-id"]) {
                q.push(("workspace_id", ws.to_string()));
            }
            if let Some(ag) = args.first(&["--agent", "--agent-id"]) {
                q.push(("agent_id", ag.to_string()));
            }
            if let Some(lim) = args.opt_usize(&["--limit"])? {
                q.push(("limit", lim.to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/ide/vault/search", &q))
                .await
        }
        "vault-index" => {
            let session_id = req_str(args, Some(0), &["--session-id", "--session"], "--session-id")?;
            let mut body = json!({ "session_id": session_id });
            if let Some(ag) = args.first(&["--agent", "--agent-id"]) {
                body["agent_id"] = json!(ag);
            }
            if let Some(ws) = args.first(&["--workspace", "--workspace-id"]) {
                body["workspace_id"] = json!(ws);
            }
            if let Some(fp) = args.first(&["--file", "--file-path"]) {
                body["file_path"] = json!(fp);
            }
            if let Some(msgs) = args.first(&["--messages-json"]) {
                body["messages"] = load_json_file_or_inline(msgs, "messages")?;
            }
            http.post("/api/v1/orca/ide/vault/index", &body).await
        }
        "plugin-list" => http.get("/api/v1/orca/ide/plugins").await,
        "plugin-validate" => {
            let mut body = load_plugin_manifest_body(args)?;
            body.insert("action".into(), json!("validate"));
            http.post("/api/v1/orca/ide/plugins", &Value::Object(body))
                .await
        }
        "plugin-install" => {
            let mut body = load_plugin_manifest_body(args)?;
            body.insert("action".into(), json!("install"));
            if let Some(fp_raw) = args.first(&["--approve"])
                && !matches!(
                    fp_raw.trim().to_ascii_lowercase().as_str(),
                    "true" | "false" | "1" | "0"
                )
            {
                // Explicit capability fingerprint passed: install first, then approve with exact fingerprint.
                body.insert("approve".into(), json!(false));
                body.insert("enforce_approval".into(), json!(false));
                let installed = http
                    .post("/api/v1/orca/ide/plugins", &Value::Object(body))
                    .await?;
                let plugin_id = installed
                    .get("manifest")
                    .and_then(|m| m.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| orca_usage("Installed plugin response missing manifest.id"))?;
                let decision = http
                    .post(
                        "/api/v1/orca/ide/plugins",
                        &json!({
                            "action": "approve",
                            "plugin_id": plugin_id,
                            "fingerprint": fp_raw.trim(),
                        }),
                    )
                    .await?;
                return Ok(json!({
                    "installed": installed,
                    "approval": decision,
                }));
            }
            let approve_bool = opts.approve
                || args.opt_bool(&["--approve"])?.unwrap_or(false)
                || args.has_bool(&["--auto-approve"]);
            body.insert("approve".into(), json!(approve_bool));
            if let Some(enf) = args.opt_bool(&["--enforce-approval"])? {
                body.insert("enforce_approval".into(), json!(enf));
            } else {
                body.insert("enforce_approval".into(), json!(true));
            }
            http.post("/api/v1/orca/ide/plugins", &Value::Object(body))
                .await
        }
        "plugin-approve" => {
            let plugin_id = req_str(args, Some(0), &["--plugin-id", "--id"], "<plugin_id>")?;
            let fingerprint = req_str(args, Some(1), &["--fingerprint", "--approve"], "<fingerprint>")?;
            http.post(
                "/api/v1/orca/ide/plugins",
                &json!({
                    "action": "approve",
                    "plugin_id": plugin_id,
                    "fingerprint": fingerprint,
                }),
            )
            .await
        }
        "plugin-evaluate" => {
            let plugin_id = req_str(args, Some(0), &["--plugin-id", "--id"], "<plugin_id>")?;
            http.post(
                "/api/v1/orca/ide/plugins",
                &json!({
                    "action": "evaluate",
                    "plugin_id": plugin_id,
                }),
            )
            .await
        }
        _ => execute_ide_runtime(http, opts, sub, args).await,
    }
}

fn parse_target_kind(raw: &str) -> Result<ComputerUseTargetKind, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "normal" | "normal_window" => Ok(ComputerUseTargetKind::NormalWindow),
        "password" | "password_field" => Ok(ComputerUseTargetKind::PasswordField),
        "security" | "system_security_dialog" => Ok(ComputerUseTargetKind::SystemSecurityDialog),
        "keychain" | "keychain_prompt" => Ok(ComputerUseTargetKind::KeychainPrompt),
        "unknown" => Ok(ComputerUseTargetKind::Unknown),
        other => Err(orca_usage(format!("Unknown --target-kind '{other}'"))),
    }
}

fn build_key_chord(args: &OrcaArgBag) -> Result<KeyChord, CliError> {
    if let Some(chord_raw) = args.first(&["--chord"]).or_else(|| {
        args.pos(0)
            .filter(|s| s.contains('+') && args.first(&["--key"]).is_none())
    }) {
        return KeyChord::parse(chord_raw)
            .ok_or_else(|| orca_usage(format!("Invalid key chord '{chord_raw}'")));
    }
    let key = req_str(args, Some(0), &["--key"], "--key")?;
    let mut mods = KeyModifiers::default();
    for raw_mod in args.all(&["--mod", "--modifier"]) {
        for part in raw_mod.split(['+', ',']) {
            match part.trim().to_ascii_lowercase().as_str() {
                "cmd" | "command" | "meta" | "mod" => mods.cmd = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "opt" | "option" => mods.alt = true,
                "shift" => mods.shift = true,
                "super" | "win" | "windows" => mods.super_key = true,
                "" => {}
                other => return Err(orca_usage(format!("Unknown modifier '{other}'"))),
            }
        }
    }
    if key.contains('+') && mods == KeyModifiers::default() {
        KeyChord::parse(key).ok_or_else(|| orca_usage(format!("Invalid key chord '{key}'")))
    } else {
        Ok(KeyChord::new(mods, key))
    }
}

async fn execute_ide_runtime(
    http: &OrcaCliHttp,
    opts: &GlobalOptions,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "computer-hotkey" => {
            let target_id = args
                .first(&["--target-id", "--target"])
                .unwrap_or("window-1")
                .to_string();
            let target_kind = match args.first(&["--target-kind"]) {
                Some(raw) => parse_target_kind(raw)?,
                None => ComputerUseTargetKind::NormalWindow,
            };
            let chord = build_key_chord(args)?;
            let req = ComputerUseRequest {
                target_id: target_id.clone(),
                target_kind,
                action: ComputerUseAction::Hotkey { chord },
            };
            let has_policy = args.has_bool(&[
                "--allow-system-shortcut",
                "--approve-target",
                "--allow-foreground-restore",
            ]);
            let body = if has_policy {
                let mut approved_targets = BTreeSet::new();
                if args.has_bool(&["--approve-target"]) {
                    approved_targets.insert(target_id);
                }
                let policy = ComputerUsePolicy {
                    allow_system_shortcut: args.has_bool(&["--allow-system-shortcut"]),
                    allow_foreground_restore: args.has_bool(&["--allow-foreground-restore"]),
                    approved_targets,
                    ..ComputerUsePolicy::default()
                };
                json!({ "request": req, "policy": policy })
            } else {
                serde_json::to_value(&req)
                    .map_err(|e| orca_usage(format!("Failed serializing request: {e}")))?
            };
            http.post("/api/v1/orca/ide/computer/guard", &body).await
        }
        "computer-guard" => {
            let raw = req_str(args, Some(0), &["--json-input", "--request-json"], "<request_json>")?;
            let body = load_json_file_or_inline(raw, "computer guard request")?;
            http.post("/api/v1/orca/ide/computer/guard", &body).await
        }
        "artifact-list" => {
            let mut q = Vec::new();
            if let Some(pid) = args.first(&["--profile-id", "--profile"]) {
                q.push(("profile_id", pid.to_string()));
            }
            if let Some(aid) = args.first(&["--artifact-id", "--id"]).or_else(|| args.pos(0)) {
                q.push(("artifact_id", aid.to_string()));
            }
            let mut val = http
                .get(&build_query_url("/api/v1/orca/ide/artifacts", &q))
                .await?;
            sanitize_artifact_value(&mut val);
            Ok(val)
        }
        "artifact-share" => {
            let title = req_str(args, Some(0), &["--title", "--name"], "--title")?;
            let content = if let Some(c) = args.first(&["--content"]) {
                c.to_string()
            } else if let Some(fp) = args.first(&["--file"]) {
                std::fs::read_to_string(fp).map_err(|e| {
                    orca_usage(format!("Failed reading artifact file '{fp}': {e}"))
                })?
            } else if let Some(p) = args.pos(1) {
                p.to_string()
            } else {
                String::new()
            };
            let confirmed = opts.approve
                || args.has_bool(&["--confirm", "--user-confirmed-share"])
                || args.opt_bool(&["--approve"])?.unwrap_or(false);
            let mut body = json!({
                "title": title,
                "content": content,
                "user_confirmed_share": confirmed,
            });
            if let Some(kind) = args.first(&["--kind"]) {
                body["kind"] = json!(kind);
            }
            if let Some(pid) = args.first(&["--profile-id", "--profile"]) {
                body["profile_id"] = json!(pid);
            }
            if let Some(aid) = args.first(&["--artifact-id", "--id"]) {
                body["artifact_id"] = json!(aid);
            }
            let mut val = http.post("/api/v1/orca/ide/artifacts", &body).await?;
            sanitize_artifact_value(&mut val);
            Ok(val)
        }
        "hibernation-check" => {
            let body = if let Some(raw) = args.first(&["--json-input"]).or_else(|| args.pos(0)) {
                load_json_file_or_inline(raw, "hibernation candidate")?
            } else {
                let mut m = Map::new();
                if let Some(tid) = args.first(&["--tab-id"]) {
                    m.insert("tab_id".into(), json!(tid));
                }
                if let Some(ag) = args.first(&["--agent", "--agent-id"]) {
                    m.insert("agent_id".into(), json!(ag));
                }
                if let Some(idle) = args.opt_u64(&["--idle-duration-ms", "--idle-ms"])? {
                    m.insert("idle_duration_ms".into(), json!(idle));
                }
                if let Some(ks) = args.opt_u64(&["--keystrokes-since-done"])? {
                    m.insert("keystrokes_since_done".into(), json!(ks));
                }
                if let Some(sub_n) = args.opt_usize(&["--live-subagent-count"])? {
                    m.insert("live_subagent_count".into(), json!(sub_n));
                }
                for (flag, key) in [
                    ("--agent-state-is-done", "agent_state_is_done"),
                    ("--tab-visible", "tab_visible"),
                    ("--mobile-driver-attached", "mobile_driver_attached"),
                    ("--unsettled-orchestration-dispatch", "unsettled_orchestration_dispatch"),
                    ("--git-operation-in-progress", "git_operation_in_progress"),
                    ("--active-port-forward", "active_port_forward"),
                    ("--pending-bot-approval", "pending_bot_approval"),
                    ("--unsynced-local-snapshot", "unsynced_local_snapshot"),
                    ("--pinned-no-hibernate", "pinned_no_hibernate"),
                ] {
                    if let Some(b) = args.opt_bool(&[flag])? {
                        m.insert(key.into(), json!(b));
                    }
                }
                Value::Object(m)
            };
            http.post("/api/v1/orca/ide/hibernation/check", &body).await
        }
        "usage" => {
            let mut q = Vec::new();
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                q.push(("now_ms", ms.to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/ide/usage", &q))
                .await
        }
        "e2ee-handshake" => {
            let raw = req_str(args, Some(0), &["--json-input"], "<handshake_json>")?;
            let body = load_json_file_or_inline(raw, "e2ee handshake")?;
            http.post("/api/v1/orca/ide/e2ee/handshake", &body).await
        }
        "serve" => {
            let health = http
                .post(
                    "/api/v1/orca/rpc",
                    &json!({
                        "method": "runtime.health",
                        "params": {},
                        "protocol_version": 3,
                        "caller_scope": "cli",
                    }),
                )
                .await?;
            let caps = http
                .post(
                    "/api/v1/orca/rpc",
                    &json!({
                        "method": "runtime.capabilities",
                        "params": {},
                        "protocol_version": 3,
                        "caller_scope": "cli",
                    }),
                )
                .await?;
            Ok(json!({
                "status": "ok",
                "mode": "headless_serve",
                "server_url": http.base_url(),
                "port": args.opt_u32(&["--port"])?,
                "health": health.get("result").cloned().unwrap_or(health),
                "capabilities": caps.get("result").cloned().unwrap_or(caps),
            }))
        }
        "profile-state" | "profile-get" | "profile-put" => {
            if sub == "profile-put" || args.first(&["--put"]).is_some() || args.first(&["--payload"]).is_some() {
                let domain = req_str(args, Some(0), &["--put", "--domain"], "--put <domain>")?;
                let raw_payload = req_str(
                    args,
                    if args.first(&["--put", "--domain"]).is_some() {
                        Some(0)
                    } else {
                        Some(1)
                    },
                    &["--payload", "--json-input"],
                    "--payload",
                )?;
                let payload = load_json_file_or_inline(raw_payload, "profile payload")?;
                let mut body = json!({
                    "domain": domain,
                    "payload": payload,
                });
                if let Some(rev) = args.opt_u64(&["--expected-revision", "--revision"])? {
                    body["expected_revision"] = json!(rev);
                }
                if let Some(dv) = args.opt_u32(&["--domain-version"])? {
                    body["domain_version"] = json!(dv);
                }
                http.put("/api/v1/orca/ide/profile-state", &body).await
            } else {
                let mut q = Vec::new();
                if let Some(d) = args.first(&["--get", "--domain"]).or_else(|| args.pos(0)) {
                    q.push(("domain", d.to_string()));
                }
                http.get(&build_query_url("/api/v1/orca/ide/profile-state", &q))
                    .await
            }
        }
        "browser-design-mode" => {
            let caps = http
                .post(
                    "/api/v1/orca/rpc",
                    &json!({
                        "method": "runtime.capabilities",
                        "params": {},
                        "protocol_version": 3,
                        "caller_scope": "cli",
                    }),
                )
                .await?;
            Ok(json!({
                "design_mode": "ready",
                "server_url": http.base_url(),
                "capabilities": caps.get("result").cloned().unwrap_or(caps),
            }))
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown 'ide' subcommand '{other}'"
        ))),
    }
}
