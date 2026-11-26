//! Dual-mode desktop runtime binding (`OrcaDesktopRuntime`) owning local pure-Rust
//! `OrcaBotsFacade` + `OrcaIdeFacade` + `InMemoryWorkspaceLineage`, optional remote
//! `OrcaClient`, and the `futures::channel::oneshot` Tokio bridge for GPUI `0.2.2`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_telegram_client::OrcaClient;
use orca_bots::{InMemoryWorkspaceLineage, OrcaBotsConfig, OrcaBotsFacade};
use orca_ide::{OrcaIdeConfig, OrcaIdeFacade};

/// Indicates whether desktop Orca surfaces operate solely on local SQLite/filesystem
/// facades or mirror/reconcile mutations against a remote Fabushi server via `OrcaClient`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrcaRuntimeBindingMode {
    LocalOnly,
    RemoteSynced,
}

/// Bridges an async Tokio future back onto GPUI's foreground executor using
/// `futures::channel::oneshot` so GPUI `0.2.2` `cx.spawn` tasks never block UI frames.
pub async fn run_on_orca_tokio<F, T>(
    handle: &tokio::runtime::Handle,
    fut: F,
) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    handle.spawn(async move {
        let out = fut.await;
        let _ = tx.send(out);
    });
    rx.await.map_err(|_| "orca tokio task dropped".to_string())
}

/// Shared desktop runtime backing both `OrcaBotsWorkspace` and `OrcaIdeWorkspace`.
///
/// Always backed by real SQLite WAL / filesystem `OrcaBotsFacade` and `OrcaIdeFacade`
/// instances (zero mock or hardcoded facade data).
pub struct OrcaDesktopRuntime {
    data_dir: PathBuf,
    local_bots: Arc<OrcaBotsFacade>,
    local_ide: Arc<OrcaIdeFacade>,
    lineage: Arc<InMemoryWorkspaceLineage>,
    remote_client: Option<OrcaClient>,
    mode: OrcaRuntimeBindingMode,
    tokio_handle: tokio::runtime::Handle,
}

impl std::fmt::Debug for OrcaDesktopRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrcaDesktopRuntime")
            .field("data_dir", &self.data_dir)
            .field("mode", &self.mode)
            .field("has_remote_client", &self.remote_client.is_some())
            .finish()
    }
}

impl OrcaDesktopRuntime {
    /// Opens an isolated self-contained runtime rooted under `root` for headless M6 tests
    /// and ephemeral local workspaces (`OrcaRuntimeBindingMode::LocalOnly`).
    pub fn open_isolated(
        root: &Path,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self, String> {
        let bots_cfg = OrcaBotsConfig::isolated(root);
        let ide_cfg = OrcaIdeConfig::isolated(root);
        let local_bots = Arc::new(
            OrcaBotsFacade::open(bots_cfg).map_err(|e| format!("open orca_bots failed: {e}"))?,
        );
        let local_ide = Arc::new(
            OrcaIdeFacade::open_with_bots(ide_cfg, local_bots.clone())
                .map_err(|e| format!("open orca_ide failed: {e}"))?,
        );
        let runtime = Self {
            data_dir: root.to_path_buf(),
            local_bots,
            local_ide,
            lineage: Arc::new(InMemoryWorkspaceLineage::new()),
            remote_client: None,
            mode: OrcaRuntimeBindingMode::LocalOnly,
            tokio_handle,
        };
        let _ = runtime.block_on_local(runtime.local_bots.sync_startup());
        Ok(runtime)
    }

    /// Opens the desktop runtime under `data_dir`, attaching an optional authenticated
    /// `OrcaClient` for `RemoteSynced` operation when connected to a Fabushi server.
    pub fn open_default(
        data_dir: impl Into<PathBuf>,
        remote_client: Option<OrcaClient>,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self, String> {
        let root = data_dir.into();
        let bots_root = root.join("bots");
        let bots_data = root.join("bots_data");
        let ide_data = root.join("ide_data");
        let workspaces_root = root.join("workspaces");

        let bots_cfg = OrcaBotsConfig::new(bots_data, bots_root);
        let ide_cfg = OrcaIdeConfig::new(ide_data, workspaces_root);
        let local_bots = Arc::new(
            OrcaBotsFacade::open(bots_cfg).map_err(|e| format!("open orca_bots failed: {e}"))?,
        );
        let local_ide = Arc::new(
            OrcaIdeFacade::open_with_bots(ide_cfg, local_bots.clone())
                .map_err(|e| format!("open orca_ide failed: {e}"))?,
        );
        let mode = if remote_client.is_some() {
            OrcaRuntimeBindingMode::RemoteSynced
        } else {
            OrcaRuntimeBindingMode::LocalOnly
        };
        let runtime = Self {
            data_dir: root,
            local_bots,
            local_ide,
            lineage: Arc::new(InMemoryWorkspaceLineage::new()),
            remote_client,
            mode,
            tokio_handle,
        };
        let _ = runtime.block_on_local(runtime.local_bots.sync_startup());
        Ok(runtime)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn bots(&self) -> &Arc<OrcaBotsFacade> {
        &self.local_bots
    }

    pub fn ide(&self) -> &Arc<OrcaIdeFacade> {
        &self.local_ide
    }

    pub fn lineage(&self) -> &Arc<InMemoryWorkspaceLineage> {
        &self.lineage
    }

    pub fn remote_client(&self) -> Option<&OrcaClient> {
        self.remote_client.as_ref()
    }

    pub fn binding_mode(&self) -> OrcaRuntimeBindingMode {
        self.mode
    }

    pub fn tokio_handle(&self) -> &tokio::runtime::Handle {
        &self.tokio_handle
    }

    /// Executes a purely local async future (e.g. `BotController` `tokio::sync::Mutex` ops)
    /// synchronously without colliding with an outer Tokio `current_thread` runtime.
    pub fn block_on_local<F: Future>(&self, fut: F) -> F::Output {
        futures::executor::block_on(fut)
    }
}
