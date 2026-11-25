use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::response::Html;
use axum::routing::get;
use bot_browser::{BrowserSessionBackend, CdpBrowserBackend, ChromeLaunchConfig};
use futures::FutureExt;
use gpui::{App, AppContext, Application, AsyncApp, Entity};
use gpui_telegram_agent::fabushi::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::ProviderConfig;
use gpui_telegram_client::TelegramClient;
use gpui_telegram_client::config::ClientConfig;
use gpui_telegram_desktop::theme::TelegramAssetSource;
use gpui_telegram_desktop::views::shell::AppShell;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::browser_e2e_fixtures::MockXServer;

static TRACKED_CHROME_DIRS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn tracked_chrome_dirs() -> &'static Mutex<Vec<PathBuf>> {
    TRACKED_CHROME_DIRS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register_tracked_chrome_dir(dir: &Path) {
    let mut dirs = tracked_chrome_dirs()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let pb = dir.to_path_buf();
    if !dirs.contains(&pb) {
        dirs.push(pb);
    }
}

fn kill_chrome_processes_for_dir(dir: &Path) {
    let dir_str = dir.to_string_lossy();
    let base_str = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if dir_str.len() < 12 && !base_str.starts_with("gpui_tg_browser_e2e_") {
        return;
    }

    #[cfg(unix)]
    {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-eo", "pid,ppid,command"])
            .output()
        else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let self_pid = std::process::id();
        let text = String::from_utf8_lossy(&out.stdout);
        let mut entries: Vec<(u32, u32, &str)> = Vec::new();
        for line in text.lines().skip(1) {
            let trimmed = line.trim_start();
            let Some(pid_end) = trimmed.find(char::is_whitespace) else {
                continue;
            };
            let Ok(pid) = trimmed[..pid_end].parse::<u32>() else {
                continue;
            };
            let after_pid = trimmed[pid_end..].trim_start();
            let Some(ppid_end) = after_pid.find(char::is_whitespace) else {
                continue;
            };
            let Ok(ppid) = after_pid[..ppid_end].parse::<u32>() else {
                continue;
            };
            let cmd = after_pid[ppid_end..].trim_start();
            entries.push((pid, ppid, cmd));
        }

        let mut target_pids: Vec<u32> = Vec::new();
        for (pid, _ppid, cmd) in &entries {
            if *pid == self_pid {
                continue;
            }
            let matches_dir = cmd.contains(dir_str.as_ref())
                || (base_str.starts_with("gpui_tg_browser_e2e_") && cmd.contains(&base_str));
            if matches_dir {
                target_pids.push(*pid);
            }
        }

        let mut idx = 0;
        while idx < target_pids.len() {
            let parent = target_pids[idx];
            idx += 1;
            for (pid, ppid, _cmd) in &entries {
                if *ppid == parent && *pid != self_pid && !target_pids.contains(pid) {
                    target_pids.push(*pid);
                }
            }
        }

        for pid in target_pids {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .output();
        }
    }
}

pub fn reap_chrome_dir(dir: &Path) {
    kill_chrome_processes_for_dir(dir);
    for attempt in 0..15 {
        if !dir.exists() {
            return;
        }
        if std::fs::remove_dir_all(dir).is_ok() {
            return;
        }
        if attempt == 2 || attempt == 6 {
            kill_chrome_processes_for_dir(dir);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn reap_tracked_chrome_dirs() {
    let dirs = {
        let mut guard = tracked_chrome_dirs()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    };
    for dir in dirs {
        reap_chrome_dir(&dir);
    }
}

pub fn exit_headless_child(code: i32, use_underscore_exit: bool) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    if use_underscore_exit {
        #[cfg(unix)]
        unsafe {
            unsafe extern "C" {
                fn _exit(status: i32) -> !;
            }
            _exit(code);
        }
    }
    std::process::exit(code);
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

pub fn run_headless_gpui_body(
    target: &str,
    use_underscore_exit: bool,
    body: fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()> + 'static>>,
) -> ! {
    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let t = target_owned.clone();
                cx.spawn(async move |app: &mut AsyncApp| {
                    let app_clone = app.clone();
                    let res = std::panic::AssertUnwindSafe(body(app_clone))
                        .catch_unwind()
                        .await;
                    reap_tracked_chrome_dirs();
                    match res {
                        Ok(()) => exit_headless_child(0, use_underscore_exit),
                        Err(payload) => {
                            let msg = panic_payload_to_string(payload);
                            eprintln!("[headless async child panic] {t}: {msg}");
                            exit_headless_child(101, use_underscore_exit);
                        }
                    }
                })
                .detach();
            });
    }));
    reap_tracked_chrome_dirs();
    if let Err(payload) = outer {
        let msg = panic_payload_to_string(payload);
        eprintln!("[headless outer panic] {target}: {msg}");
        exit_headless_child(101, use_underscore_exit);
    }
    exit_headless_child(0, use_underscore_exit);
}

pub fn spawn_headless_subprocess(
    child_env: &str,
    child_target_env: &str,
    child_after_ctors_env: &str,
    test_fn_name: &str,
    after_ctors_window: bool,
    extra_home: Option<&Path>,
    extra_config_dir: Option<&Path>,
) {
    let owned_temp_cfg;
    let cfg_path = if let Some(dir) = extra_config_dir {
        dir
    } else {
        owned_temp_cfg = TempDir::new().expect("create TempDir for GPUI_TELEGRAM_CONFIG_DIR");
        owned_temp_cfg.path()
    };

    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(child_env, "1")
        .env(child_target_env, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", cfg_path)
        .env("GPUI_OFFLINE_TEST", "1");
    if let Some(home) = extra_home {
        cmd.env("HOME", home);
    }
    if after_ctors_window {
        cmd.env(child_after_ctors_env, "1")
            .arg("--exact")
            .arg("__e2e_headless_child_noop__");
    } else {
        cmd.arg("--exact").arg(test_fn_name);
    }
    let output = cmd
        .arg("--nocapture")
        .output()
        .expect("spawn headless gpui child");

    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        if line.contains("[T4-05 Latency & SPA Audit]") {
            eprintln!("{line}");
        }
    }

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "headless child test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }
}

pub fn new_isolated_client(server_url: &str) -> (TempDir, Arc<TelegramClient>) {
    let dir = TempDir::new().expect("temp dir for client");
    let client = open_client_in_dir(server_url, dir.path());
    (dir, client)
}

pub fn open_client_in_dir(server_url: &str, dir: &Path) -> Arc<TelegramClient> {
    let cfg = ClientConfig {
        server_url: server_url.to_string(),
        ..Default::default()
    };
    Arc::new(TelegramClient::open_with_dir(cfg, dir).expect("TelegramClient::open_with_dir"))
}

pub fn build_authenticated_shell(
    app: &mut App,
    client: Arc<TelegramClient>,
    user_id: u64,
    username: &str,
) -> Entity<AppShell> {
    let _rt_guard = client.runtime().enter();
    app.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.is_authenticated = true;
        s.session_sync.user_id = Some(user_id);
        s.session_sync.username = Some(username.to_string());
        s.session_sync.session_token = Some(format!("tok_{username}"));
        s.magpie_local.bootstrapped_auth_generation = Some(s.session_sync.auth_generation);
        s
    })
}

pub fn build_authenticated_shell_with_token(
    app: &mut App,
    client: Arc<TelegramClient>,
    user_id: u64,
    username: &str,
    token: &str,
) -> Entity<AppShell> {
    let _rt_guard = client.runtime().enter();
    app.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.is_authenticated = true;
        s.session_sync.user_id = Some(user_id);
        s.session_sync.username = Some(username.to_string());
        s.session_sync.session_token = Some(token.to_string());
        s.magpie_local.bootstrapped_auth_generation = Some(s.session_sync.auth_generation);
        s
    })
}

pub async fn pump_until(
    app: &mut AsyncApp,
    timeout: Duration,
    mut predicate: impl FnMut(&mut App) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let done = app.update(|cx| predicate(cx)).unwrap_or(false);
        if done {
            return true;
        }
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
    }
    app.update(|cx| predicate(cx)).unwrap_or(false)
}

/// Pumps GPUI ticks at `tick_interval` until `on_tick(cx, tick_idx)` returns `true` or `timeout` elapses.
/// Returns `(completed, tick_count, max_tick_gap_ms)`.
pub async fn pump_with_ticks(
    app: &mut AsyncApp,
    timeout: Duration,
    tick_interval: Duration,
    mut on_tick: impl FnMut(&mut App, usize) -> bool,
) -> (bool, usize, u64) {
    let start = Instant::now();
    let mut last_tick = Instant::now();
    let mut max_gap_ms: u64 = 0;
    let mut ticks: usize = 0;
    while start.elapsed() < timeout {
        let gap = last_tick.elapsed().as_millis() as u64;
        if ticks > 0 && gap > max_gap_ms {
            max_gap_ms = gap;
        }
        last_tick = Instant::now();
        ticks += 1;
        let done = app.update(|cx| on_tick(cx, ticks)).unwrap_or(false);
        if done {
            return (true, ticks, max_gap_ms);
        }
        app.background_executor().timer(tick_interval).await;
    }
    let done = app.update(|cx| on_tick(cx, ticks + 1)).unwrap_or(false);
    (done, ticks, max_gap_ms)
}

pub async fn launch_chrome_in(dir: &Path) -> CdpBrowserBackend {
    register_tracked_chrome_dir(dir);
    CdpBrowserBackend::launch_chrome(ChromeLaunchConfig::new(dir))
        .await
        .expect("headless Chrome must launch for GPUI E2E tests")
}

pub async fn close_chrome_and_reap(
    chrome: &CdpBrowserBackend,
    dir: &Path,
) -> Result<(), bot_browser::BotBrowserError> {
    let res = chrome.close().await;
    reap_chrome_dir(dir);
    let mut guard = tracked_chrome_dirs()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.retain(|p| p != dir);
    res
}

pub async fn wait_for_url_contains(
    backend: &dyn BrowserSessionBackend,
    needle: &str,
    timeout: Duration,
) -> String {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(url) = backend.current_url().await {
            if url.contains(needle) {
                return url;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for current_url to contain '{needle}'"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub async fn wait_for_tweet_count(server: &MockXServer, expected: usize, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if server.tweet_count() >= expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for MockXServer tweet_count >= {expected} (got {})",
            server.tweet_count()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub async fn query_element_center_css(
    backend: &dyn BrowserSessionBackend,
    selector: &str,
) -> (f64, f64) {
    let selector_json = serde_json::to_string(selector).expect("serialize selector");
    let script = format!(
        "(() => {{ const el = document.querySelector({selector_json}); if (!el) return null; el.scrollIntoView({{block: 'center', inline: 'center'}}); const r = el.getBoundingClientRect(); return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }}; }})()"
    );
    let val = backend
        .eval_js(&script)
        .await
        .expect("eval_js element rect");
    let x = val["x"]
        .as_f64()
        .unwrap_or_else(|| panic!("missing x for selector {selector}: {val:?}"));
    let y = val["y"]
        .as_f64()
        .unwrap_or_else(|| panic!("missing y for selector {selector}: {val:?}"));
    (x, y)
}

pub fn configure_host_with_mock_llm(host: &FabushiBotHost, llm_base_url: &str) {
    let engine = host.core_engine();
    engine.providers().register_provider(ProviderConfig::openai_compatible(
        "mock_openai",
        "Mock OpenAI",
        llm_base_url,
        "mock-gpt",
        "sk-mock-test",
    ));
    engine
        .providers()
        .set_active("mock_openai", Some("mock-gpt"))
        .expect("set active mock_openai provider");
}

pub struct DelayedHtmlServer {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl DelayedHtmlServer {
    pub async fn start(delay_ms: u64) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let router = Router::new()
            .route(
                "/slow",
                get(move || async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    Html(
                        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8" /><title>Slow Loaded Page</title></head>
<body><main><h1 data-testid="slow-ready">Slow Load Complete</h1></main></body>
</html>"#,
                    )
                }),
            )
            .route(
                "/fast",
                get(|| async {
                    Html(
                        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8" /><title>Fast Page</title></head>
<body><main><h1 data-testid="fast-ready">Fast Ready</h1></main></body>
</html>"#,
                    )
                }),
            );
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
            join: Some(join),
        })
    }

    pub fn url(&self, path: &str) -> String {
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("http://{}{}", self.addr, path)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for DelayedHtmlServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}
