//! Shared Stdio MCP Child Process Multiplexer (`SharedMcpPool`):
//! - Canonical SHA-256 hex fingerprint of `[command, args, sorted_env]` (`compute_server_fingerprint`).
//! - `drop_inherited_orca_identity_env`: strips inherited `ORCA_*`, `FABUSHI_*`,
//!   `GPUI_TELEGRAM_*`, `GPUI_TG_*` env vars via `env_clear()` + safe baseline env + explicit server env.
//! - Auto-spawns stdio child processes (`tokio::process::Command` with `kill_on_drop(true)`),
//!   multiplexes concurrent newline-delimited JSON-RPC requests correlated by `id`,
//!   auto-restarts and reaps old processes on logical server config fingerprint change.
//! - 10s Failed-Start Cache (`FAILED_START_MS = 10_000`) debouncing spawn failures & startup crashes.
//! - Clean `shutdown_server` and `shutdown_all` terminating and awaiting child exit (zero leaks).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orca_core::bot_types::is_valid_bot_tool_env_name;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

use crate::{OrcaBotsError, Result};

/// Failed-start debounce window in milliseconds (10 seconds).
pub const FAILED_START_MS: u64 = 10_000;
/// Default per-request JSON-RPC timeout in milliseconds (30 seconds).
pub const DEFAULT_MCP_RPC_TIMEOUT_MS: u64 = 30_000;

const BLOCKED_INHERITED_ENV_PREFIXES: [&str; 4] = [
    "ORCA_",
    "FABUSHI_",
    "GPUI_TELEGRAM_",
    "GPUI_TG_",
];

const SAFE_BASELINE_ENV_KEYS: [&str; 17] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "TEMP",
    "TMP",
    "XDG_RUNTIME_DIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "TZ",
    "NO_COLOR",
];

/// Configuration for a shared host-level stdio MCP child server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMcpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

impl SharedMcpServerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.command.trim().is_empty() {
            return Err(OrcaBotsError::mcp(
                "invalid_config",
                "MCP server command must be non-empty.",
            ));
        }
        for key in self.env.keys() {
            if !is_valid_bot_tool_env_name(key) {
                return Err(OrcaBotsError::mcp(
                    "invalid_env_name",
                    format!("Invalid POSIX environment variable name '{key}'."),
                ));
            }
        }
        Ok(())
    }
}

/// Computes the canonical lowercase SHA-256 hex fingerprint of `[command, args, sorted_env]`
/// (plus `cwd` when explicitly configured).
pub fn compute_server_fingerprint(config: &SharedMcpServerConfig) -> String {
    let sorted_env: Vec<(&str, &str)> = config
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let canonical = match &config.cwd {
        Some(cwd) => serde_json::json!([
            &config.command,
            &config.args,
            sorted_env,
            cwd.to_string_lossy()
        ]),
        None => serde_json::json!([&config.command, &config.args, sorted_env]),
    };
    let bytes = canonical.to_string();
    hex::encode(Sha256::digest(bytes.as_bytes()))
}

/// Strips inherited `ORCA_*`, `FABUSHI_*`, `GPUI_TELEGRAM_*`, and `GPUI_TG_*`
/// identity variables from a key-value environment iterator.
pub fn drop_inherited_orca_identity_env<I, K, V>(inherited: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let mut out = BTreeMap::new();
    for (k, v) in inherited {
        let key = k.into();
        let upper = key.to_ascii_uppercase();
        if BLOCKED_INHERITED_ENV_PREFIXES
            .iter()
            .any(|prefix| upper.starts_with(prefix))
        {
            continue;
        }
        out.insert(key, v.into());
    }
    out
}

/// Builds a sanitized child environment: starts from `env_clear()`, keeps ONLY safe
/// baseline host env vars after stripping Orca/Fabushi/GPUI identity vars, and overlays
/// explicit `config.env` entries.
pub fn build_child_environment(
    inherited: &[(String, String)],
    explicit_env: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let filtered = drop_inherited_orca_identity_env(
        inherited
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    let mut out = BTreeMap::new();
    for safe_key in SAFE_BASELINE_ENV_KEYS {
        if let Some(val) = filtered.get(safe_key) {
            out.insert(safe_key.to_string(), val.clone());
        }
    }
    for (k, v) in explicit_env {
        out.insert(k.clone(), v.clone());
    }
    out
}

/// Snapshot metadata about a live child process inside `SharedMcpPool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedMcpChildInfo {
    pub fingerprint: String,
    pub pid: Option<u32>,
    pub alive: bool,
}

#[derive(Debug, Clone)]
struct FailedStartEntry {
    failed_at: Instant,
    reason: String,
}

type PendingWaiterMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonValue>>>>>;

struct SharedMcpChild {
    fingerprint: String,
    pid: Option<u32>,
    alive: Arc<AtomicBool>,
    ever_responded: Arc<AtomicBool>,
    next_id: AtomicU64,
    stdin_tx: mpsc::Sender<String>,
    pending: PendingWaiterMap,
    child: Arc<AsyncMutex<Option<Child>>>,
    reader_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    writer_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SharedMcpChild {
    fn spawn(config: &SharedMcpServerConfig, fingerprint: String) -> Result<Arc<Self>> {
        config.validate()?;
        let inherited: Vec<(String, String)> = std::env::vars().collect();
        let child_env = build_child_environment(&inherited, &config.env);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .env_clear()
            .envs(child_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd.spawn().map_err(|err| {
            OrcaBotsError::mcp(
                "spawn_failed",
                format!("Failed to spawn MCP server '{}': {err}", config.command),
            )
        })?;
        let pid = child.id();
        let mut stdin = child.stdin.take().ok_or_else(|| {
            OrcaBotsError::mcp("spawn_failed", "MCP child stdin pipe unavailable.")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            OrcaBotsError::mcp("spawn_failed", "MCP child stdout pipe unavailable.")
        })?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(256);
        let pending: PendingWaiterMap = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let ever_responded = Arc::new(AtomicBool::new(false));

        let writer_alive = alive.clone();
        let writer_task = tokio::spawn(async move {
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    writer_alive.store(false, Ordering::SeqCst);
                    break;
                }
            }
        });

        let reader_pending = pending.clone();
        let reader_alive = alive.clone();
        let reader_ever = ever_responded.clone();
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(val) = serde_json::from_str::<JsonValue>(trimmed) else {
                    continue;
                };
                let Some(id) = val.get("id").and_then(JsonValue::as_u64) else {
                    continue;
                };
                reader_ever.store(true, Ordering::SeqCst);
                let waiter = reader_pending
                    .lock()
                    .ok()
                    .and_then(|mut map| map.remove(&id));
                if let Some(tx) = waiter {
                    if let Some(err_obj) = val.get("error") {
                        let msg = err_obj
                            .get("message")
                            .and_then(JsonValue::as_str)
                            .unwrap_or("MCP server returned JSON-RPC error");
                        let _ = tx.send(Err(OrcaBotsError::mcp("rpc_error", msg.to_string())));
                    } else {
                        let result = val.get("result").cloned().unwrap_or(JsonValue::Null);
                        let _ = tx.send(Ok(result));
                    }
                }
            }
            reader_alive.store(false, Ordering::SeqCst);
            if let Ok(mut map) = reader_pending.lock() {
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(OrcaBotsError::mcp(
                        "child_exited",
                        "MCP child process closed stdout before responding.",
                    )));
                }
            }
        });

        Ok(Arc::new(Self {
            fingerprint,
            pid,
            alive,
            ever_responded,
            next_id: AtomicU64::new(1),
            stdin_tx,
            pending,
            child: Arc::new(AsyncMutex::new(Some(child))),
            reader_task: Mutex::new(Some(reader_task)),
            writer_task: Mutex::new(Some(writer_task)),
        }))
    }

    async fn call(&self, method: &str, params: JsonValue, timeout: Duration) -> Result<JsonValue> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(OrcaBotsError::mcp(
                "child_exited",
                "MCP child process is no longer alive.",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().map_err(|_| {
                OrcaBotsError::mcp("lock_poisoned", "MCP pending waiter mutex poisoned.")
            })?;
            map.insert(id, tx);
        }

        let req_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })
        .to_string();

        if self.stdin_tx.send(req_line).await.is_err() {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&id);
            }
            self.alive.store(false, Ordering::SeqCst);
            return Err(OrcaBotsError::mcp(
                "child_exited",
                "Failed writing JSON-RPC request to MCP child stdin.",
            ));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(OrcaBotsError::mcp(
                "child_exited",
                "MCP child process terminated before replying.",
            )),
            Err(_) => {
                if let Ok(mut map) = self.pending.lock() {
                    map.remove(&id);
                }
                Err(OrcaBotsError::mcp(
                    "rpc_timeout",
                    format!("MCP JSON-RPC call '{method}' timed out."),
                ))
            }
        }
    }

    async fn terminate_and_wait(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Ok(mut map) = self.pending.lock() {
            for (_, tx) in map.drain() {
                let _ = tx.send(Err(OrcaBotsError::mcp(
                    "server_shutdown",
                    "MCP server was shut down.",
                )));
            }
        }
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Some(h) = self.writer_task.lock().ok().and_then(|mut s| s.take()) {
            h.abort();
        }
        if let Some(h) = self.reader_task.lock().ok().and_then(|mut s| s.take()) {
            h.abort();
        }
    }
}

struct PoolInner {
    logical_servers: HashMap<String, String>,
    processes: HashMap<String, Arc<SharedMcpChild>>,
    failed_starts: HashMap<String, FailedStartEntry>,
}

/// Shared host-level stdio MCP child process pool with SHA-256 deduplication,
/// config-change auto-restart, inherited identity-env stripping, and 10s failed-start debounce.
#[derive(Clone)]
pub struct SharedMcpPool {
    inner: Arc<AsyncMutex<PoolInner>>,
    failed_start_ttl: Duration,
    rpc_timeout: Duration,
}

impl std::fmt::Debug for SharedMcpPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedMcpPool")
            .field("failed_start_ttl", &self.failed_start_ttl)
            .field("rpc_timeout", &self.rpc_timeout)
            .finish()
    }
}

impl Default for SharedMcpPool {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedMcpPool {
    /// Creates a new `SharedMcpPool` with default `FAILED_START_MS` (10s) debounce
    /// and `DEFAULT_MCP_RPC_TIMEOUT_MS` (30s) RPC timeout.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AsyncMutex::new(PoolInner {
                logical_servers: HashMap::new(),
                processes: HashMap::new(),
                failed_starts: HashMap::new(),
            })),
            failed_start_ttl: Duration::from_millis(FAILED_START_MS),
            rpc_timeout: Duration::from_millis(DEFAULT_MCP_RPC_TIMEOUT_MS),
        }
    }

    /// Configures a custom failed-start debounce window (useful for deterministic expiry tests).
    pub fn with_failed_start_ttl(mut self, ttl: Duration) -> Self {
        self.failed_start_ttl = ttl;
        self
    }

    /// Configures a custom JSON-RPC call timeout.
    pub fn with_rpc_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_timeout = timeout;
        self
    }

    /// Returns `true` if `fingerprint` currently sits in the active failed-start debounce cache.
    pub async fn is_failed_start_cached(&self, fingerprint: &str) -> bool {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        inner
            .failed_starts
            .retain(|_, e| now.saturating_duration_since(e.failed_at) < self.failed_start_ttl);
        inner.failed_starts.contains_key(fingerprint)
    }

    /// Clears a cached failed-start entry explicitly (e.g. after user repairs a binary).
    pub async fn clear_failed_start_cache(&self, fingerprint: &str) {
        let mut inner = self.inner.lock().await;
        inner.failed_starts.remove(fingerprint);
    }

    /// Returns the number of currently active child processes tracked by the pool.
    pub async fn active_process_count(&self) -> usize {
        let inner = self.inner.lock().await;
        inner
            .processes
            .values()
            .filter(|c| c.alive.load(Ordering::SeqCst))
            .count()
    }

    /// Returns live process info for a logical `server_name`, if currently spawned.
    pub async fn server_info(&self, server_name: &str) -> Option<SharedMcpChildInfo> {
        let inner = self.inner.lock().await;
        let fp = inner.logical_servers.get(server_name)?;
        let child = inner.processes.get(fp)?;
        Some(SharedMcpChildInfo {
            fingerprint: child.fingerprint.clone(),
            pid: child.pid,
            alive: child.alive.load(Ordering::SeqCst),
        })
    }

    /// Invokes a newline-delimited JSON-RPC `method` on `server_name`, sharing an
    /// existing process if `[command, args, sorted_env]` matches by SHA-256 fingerprint,
    /// auto-restarting and terminating the old child if `server_name`'s config changed,
    /// and debouncing spawn/startup crashes for `FAILED_START_MS` (10s).
    pub async fn call(
        &self,
        server_name: &str,
        config: &SharedMcpServerConfig,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue> {
        if server_name.trim().is_empty() {
            return Err(OrcaBotsError::mcp(
                "invalid_server_name",
                "Logical MCP server name must be non-empty.",
            ));
        }
        config.validate()?;
        let fp = compute_server_fingerprint(config);

        let (child_to_use, old_child_to_reap) = {
            let mut inner = self.inner.lock().await;
            let now = Instant::now();
            inner
                .failed_starts
                .retain(|_, e| now.saturating_duration_since(e.failed_at) < self.failed_start_ttl);

            // Detect config change for `server_name` and reap orphaned previous child.
            let prev_fp = inner.logical_servers.insert(server_name.to_string(), fp.clone());
            let mut orphan: Option<Arc<SharedMcpChild>> = None;
            if let Some(old_fp) = prev_fp
                && old_fp != fp
            {
                let still_referenced = inner.logical_servers.values().any(|v| v == &old_fp);
                if !still_referenced {
                    orphan = inner.processes.remove(&old_fp);
                }
            }

            if let Some(entry) = inner.failed_starts.get(&fp) {
                let reason = entry.reason.clone();
                drop(inner);
                if let Some(old) = orphan {
                    old.terminate_and_wait().await;
                }
                return Err(OrcaBotsError::mcp(
                    "failed_start_cached",
                    format!(
                        "MCP server start debounced for {}ms after recent failure: {reason}",
                        self.failed_start_ttl.as_millis()
                    ),
                ));
            }

            // Reuse existing live child for this fingerprint if present.
            let existing = inner
                .processes
                .get(&fp)
                .filter(|c| c.alive.load(Ordering::SeqCst))
                .cloned();

            let child = match existing {
                Some(c) => c,
                None => {
                    if let Some(dead) = inner.processes.remove(&fp) {
                        drop(inner);
                        dead.terminate_and_wait().await;
                        inner = self.inner.lock().await;
                    }
                    match SharedMcpChild::spawn(config, fp.clone()) {
                        Ok(spawned) => {
                            inner.processes.insert(fp.clone(), spawned.clone());
                            spawned
                        }
                        Err(err) => {
                            inner.failed_starts.insert(
                                fp.clone(),
                                FailedStartEntry {
                                    failed_at: Instant::now(),
                                    reason: err.to_string(),
                                },
                            );
                            drop(inner);
                            if let Some(old) = orphan {
                                old.terminate_and_wait().await;
                            }
                            return Err(err);
                        }
                    }
                }
            };
            (child, orphan)
        };

        if let Some(old) = old_child_to_reap {
            old.terminate_and_wait().await;
        }

        let res = child_to_use.call(method, params, self.rpc_timeout).await;
        if let Err(ref err) = res
            && !child_to_use.alive.load(Ordering::SeqCst)
        {
            // Child died; remove from live pool and cache startup crash for FAILED_START_MS if it
            // never completed a single JSON-RPC response!
            let was_startup_crash = !child_to_use.ever_responded.load(Ordering::SeqCst);
            let mut inner = self.inner.lock().await;
            if inner
                .processes
                .get(&fp)
                .is_some_and(|c| Arc::ptr_eq(c, &child_to_use))
            {
                inner.processes.remove(&fp);
            }
            if was_startup_crash {
                inner.failed_starts.insert(
                    fp,
                    FailedStartEntry {
                        failed_at: Instant::now(),
                        reason: err.to_string(),
                    },
                );
            }
            drop(inner);
            child_to_use.terminate_and_wait().await;
        }
        res
    }

    /// Shuts down `server_name` and terminates its child process if no other logical
    /// server still shares that fingerprint.
    pub async fn shutdown_server(&self, server_name: &str) -> bool {
        let orphan = {
            let mut inner = self.inner.lock().await;
            let Some(fp) = inner.logical_servers.remove(server_name) else {
                return false;
            };
            let still_referenced = inner.logical_servers.values().any(|v| v == &fp);
            if still_referenced {
                None
            } else {
                inner.processes.remove(&fp)
            }
        };
        if let Some(child) = orphan {
            child.terminate_and_wait().await;
        }
        true
    }

    /// Terminates and awaits exit of ALL managed stdio MCP child processes (zero leaked processes).
    pub async fn shutdown_all(&self) {
        let children: Vec<Arc<SharedMcpChild>> = {
            let mut inner = self.inner.lock().await;
            inner.logical_servers.clear();
            inner.processes.drain().map(|(_, c)| c).collect()
        };
        for child in children {
            child.terminate_and_wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Tiny POSIX `/bin/sh` JSON-RPC 2.0 echo server replying with pid, env tag,
    /// leaked identity var count, and echoed request id.
    const SH_JSONRPC_ECHO: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  leaked=$(env | grep -E '^(ORCA_|FABUSHI_|GPUI_TELEGRAM_|GPUI_TG_)' | wc -l | tr -d ' ')
  printf '{"jsonrpc":"2.0","id":%s,"result":{"pid":%s,"tag":"%s","leaked":%s}}\n' "${id:-0}" "$$" "${MCP_TEST_TAG:-none}" "$leaked"
done
"#;

    #[test]
    fn fingerprint_canonical_and_drop_inherited_orca_identity_env() {
        let mut env_a = BTreeMap::new();
        env_a.insert("B_VAR".into(), "2".into());
        env_a.insert("A_VAR".into(), "1".into());
        let mut env_b = BTreeMap::new();
        env_b.insert("A_VAR".into(), "1".into());
        env_b.insert("B_VAR".into(), "2".into());

        let cfg_a = SharedMcpServerConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            env: env_a,
            cwd: None,
        };
        let cfg_b = SharedMcpServerConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            env: env_b,
            cwd: None,
        };
        let fp_a = compute_server_fingerprint(&cfg_a);
        let fp_b = compute_server_fingerprint(&cfg_b);
        assert_eq!(fp_a.len(), 64);
        assert_eq!(fp_a, fp_b);

        let inherited = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), "/tmp/home".into()),
            ("ORCA_BOT_TOKEN".into(), "secret-1".into()),
            ("orca_session_id".into(), "secret-2".into()),
            ("FABUSHI_RELAY_KEY".into(), "secret-3".into()),
            ("GPUI_TELEGRAM_TOKEN".into(), "secret-4".into()),
            ("GPUI_TG_SECRET".into(), "secret-5".into()),
            ("UNSAFE_RANDOM_VAR".into(), "should-not-leak-baseline".into()),
        ];
        let dropped = drop_inherited_orca_identity_env(inherited.clone());
        assert!(!dropped.contains_key("ORCA_BOT_TOKEN"));
        assert!(!dropped.contains_key("orca_session_id"));
        assert!(!dropped.contains_key("FABUSHI_RELAY_KEY"));
        assert!(!dropped.contains_key("GPUI_TELEGRAM_TOKEN"));
        assert!(!dropped.contains_key("GPUI_TG_SECRET"));
        assert_eq!(dropped.get("PATH").map(String::as_str), Some("/usr/bin:/bin"));

        let mut explicit = BTreeMap::new();
        explicit.insert("MCP_TOOL_MODE".into(), "strict".into());
        let child_env = build_child_environment(&inherited, &explicit);
        assert_eq!(child_env.get("PATH").map(String::as_str), Some("/usr/bin:/bin"));
        assert_eq!(child_env.get("HOME").map(String::as_str), Some("/tmp/home"));
        assert_eq!(child_env.get("MCP_TOOL_MODE").map(String::as_str), Some("strict"));
        assert!(!child_env.contains_key("UNSAFE_RANDOM_VAR"));
        assert!(!child_env.contains_key("ORCA_BOT_TOKEN"));
        assert!(!child_env.contains_key("FABUSHI_RELAY_KEY"));
    }

    #[tokio::test]
    async fn shared_mcp_multiplexing_sha256_sharing_and_config_change_restart() {
        let pool = SharedMcpPool::new();

        let mut env1 = BTreeMap::new();
        env1.insert("MCP_TEST_TAG".into(), "v1".into());
        let cfg_v1 = SharedMcpServerConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), SH_JSONRPC_ECHO.into()],
            env: env1,
            cwd: None,
        };

        // Two logical servers sharing identical config share ONE physical child process (same PID),
        // even under concurrent multiplexed JSON-RPC calls.
        let (r1, r2, r3) = tokio::join!(
            pool.call("bot-a-docs", &cfg_v1, "tools/list", serde_json::json!({})),
            pool.call("bot-b-docs", &cfg_v1, "tools/list", serde_json::json!({})),
            pool.call("bot-a-docs", &cfg_v1, "ping", serde_json::json!({"n": 3})),
        );
        let v1_a = r1.unwrap();
        let v1_b = r2.unwrap();
        let v1_c = r3.unwrap();
        let pid_v1 = v1_a["pid"].as_u64().unwrap();
        assert_eq!(v1_b["pid"].as_u64().unwrap(), pid_v1);
        assert_eq!(v1_c["pid"].as_u64().unwrap(), pid_v1);
        assert_eq!(v1_a["tag"].as_str(), Some("v1"));
        assert_eq!(v1_a["leaked"].as_u64(), Some(0));
        assert_eq!(pool.active_process_count().await, 1);

        // Shut down bot-b-docs: process stays alive because bot-a-docs still shares the fingerprint.
        assert!(pool.shutdown_server("bot-b-docs").await);
        assert_eq!(pool.active_process_count().await, 1);

        // Change config for bot-a-docs -> spawns a NEW child process (new PID) and terminates the old one!
        let mut env2 = BTreeMap::new();
        env2.insert("MCP_TEST_TAG".into(), "v2".into());
        let cfg_v2 = SharedMcpServerConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), SH_JSONRPC_ECHO.into()],
            env: env2,
            cwd: None,
        };
        let v2_res = pool
            .call("bot-a-docs", &cfg_v2, "tools/list", serde_json::json!({}))
            .await
            .unwrap();
        let pid_v2 = v2_res["pid"].as_u64().unwrap();
        assert_ne!(pid_v2, pid_v1);
        assert_eq!(v2_res["tag"].as_str(), Some("v2"));
        assert_eq!(pool.active_process_count().await, 1);

        // Shutdown all leaves zero active processes.
        pool.shutdown_all().await;
        assert_eq!(pool.active_process_count().await, 0);
    }

    #[tokio::test]
    async fn shared_mcp_ten_second_failed_start_debounce_prevents_spawn_loop() {
        assert_eq!(FAILED_START_MS, 10_000);
        let tmp = TempDir::new().unwrap();
        let counter_file = tmp.path().join("spawns.log");
        let crash_script = format!(
            "echo spawned >> '{}'; exit 1",
            counter_file.display()
        );
        let crash_cfg = SharedMcpServerConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), crash_script],
            env: BTreeMap::new(),
            cwd: None,
        };
        let fp = compute_server_fingerprint(&crash_cfg);

        // Default pool uses FAILED_START_MS (10s).
        let pool = SharedMcpPool::new();
        let err1 = pool
            .call("crashing-tool", &crash_cfg, "initialize", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err1.to_string().contains("child_exited"));
        assert!(pool.is_failed_start_cached(&fp).await);

        // Second call within 10s window MUST hit debounce cache without spawning again!
        let err2 = pool
            .call("crashing-tool", &crash_cfg, "initialize", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err2.to_string().contains("failed_start_cached"));

        let spawn_lines = fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            spawn_lines.lines().count(),
            1,
            "crashing binary must spawn only ONCE during 10s debounce window"
        );

        // Nonexistent binary spawn failure is also cached.
        let missing_cfg = SharedMcpServerConfig {
            command: "/nonexistent/orca-missing-mcp-bin".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
        };
        let missing_fp = compute_server_fingerprint(&missing_cfg);
        let _ = pool
            .call("missing", &missing_cfg, "initialize", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(pool.is_failed_start_cached(&missing_fp).await);
        let cached_err = pool
            .call("missing", &missing_cfg, "initialize", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(cached_err.to_string().contains("failed_start_cached"));

        pool.shutdown_all().await;
    }
}
