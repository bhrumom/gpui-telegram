//! `openbot serve` handler: live `/v1/*` Team API HTTP/SSE + loopback automation server (`OB-F01`, `OB-F11`, `OB-F13`, `OB-F20`).

use std::io::Write;
use std::path::Path;

use openbot_runtime::{AutomationServerHandle, TeamApiHandle};
use serde_json::{Value, json};

use super::{FlagSpec, ParsedFlags, map_runtime_error, open_host_runtime, parse_u16_domain};
use crate::envelope::CliOutcome;
use crate::error::CliError;

pub fn serve_spec() -> FlagSpec {
    FlagSpec {
        value_flags: &["--bind", "--team-token", "--automation-port"],
        bool_flags: &[],
        opt_bool_flags: &[],
        required_flags: &[],
    }
}

pub struct StartedOpenBotServers {
    pub team_api: TeamApiHandle,
    pub automation: AutomationServerHandle,
    pub startup_json: Value,
}

impl StartedOpenBotServers {
    pub async fn shutdown(self) -> Result<(), CliError> {
        let r1 = self.team_api.shutdown().await.map_err(map_runtime_error);
        let r2 = self.automation.shutdown().await.map_err(map_runtime_error);
        r1?;
        r2
    }
}

pub fn parse_bind_port(bind: &str) -> Result<u16, CliError> {
    let trimmed = bind.trim();
    if trimmed.is_empty() {
        return Err(CliError::Validation("bind address must not be empty".into()));
    }
    if let Ok(port_only) = trimmed.parse::<u16>() {
        return Ok(port_only);
    }
    let Some((host, port_str)) = trimmed.rsplit_once(':') else {
        return Err(CliError::Validation(format!(
            "invalid bind address '{trimmed}': expected 127.0.0.1:<port>"
        )));
    };
    let host_clean = host.trim().trim_start_matches('[').trim_end_matches(']');
    if !matches!(host_clean, "127.0.0.1" | "localhost" | "::1") {
        return Err(CliError::Validation(format!(
            "bind host '{host_clean}' must be loopback (127.0.0.1, localhost, or ::1)"
        )));
    }
    parse_u16_domain(port_str, "bind port")
}

pub async fn start_serve_handles(
    data_dir: &Path,
    team_port: u16,
    automation_port: u16,
    team_token: Option<String>,
) -> Result<StartedOpenBotServers, CliError> {
    let runtime = open_host_runtime(data_dir)?;
    let team_api = runtime
        .start_team_api_server(team_port, team_token)
        .await
        .map_err(map_runtime_error)?;
    let automation = match runtime.start_automation_server(automation_port).await {
        Ok(handle) => handle,
        Err(err) => {
            let _ = team_api.shutdown().await;
            return Err(map_runtime_error(err));
        }
    };
    let startup_json = json!({
        "team_api_url": team_api.url(),
        "automation_url": automation.url(),
        "automation_token": automation.token(),
    });
    Ok(StartedOpenBotServers {
        team_api,
        automation,
        startup_json,
    })
}

pub async fn run_serve(flags: &ParsedFlags, data_dir: &Path) -> Result<CliOutcome, CliError> {
    let team_port = match flags.get("--bind") {
        Some(raw) => parse_bind_port(raw)?,
        None => 0,
    };
    let automation_port = match flags.get("--automation-port") {
        Some(raw) => parse_u16_domain(raw, "automation-port")?,
        None => 0,
    };
    let team_token = flags
        .get("--team-token")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let servers = start_serve_handles(data_dir, team_port, automation_port, team_token).await?;
    let compact_line = serde_json::to_string(&servers.startup_json)
        .map_err(|e| CliError::Execution(format!("failed to serialize serve startup JSON: {e}")))?;

    {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{compact_line}")
            .map_err(|e| CliError::Execution(format!("failed to write serve startup JSON: {e}")))?;
        out.flush()
            .map_err(|e| CliError::Execution(format!("failed to flush serve startup JSON: {e}")))?;
    }

    wait_for_shutdown_signal().await;
    let _ = servers.shutdown().await;
    Ok(CliOutcome::ok(String::new()))
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        let ctrl_c = tokio::signal::ctrl_c();
        match term.as_mut() {
            Some(sigterm) => {
                tokio::select! {
                    _ = ctrl_c => {}
                    _ = sigterm.recv() => {}
                }
            }
            None => {
                let _ = ctrl_c.await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
