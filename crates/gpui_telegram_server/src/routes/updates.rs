//! High-speed release manifest and range-cached update asset relay routes.
//!
//! Proxies official `bhrumom/gpui-telegram` GitHub Release artifacts through the
//! cloud server with strict path allowlisting, atomic disk caching, and RFC 7233
//! `206 Partial Content` streaming via `stream_cas_file`.

use crate::media::streaming::stream_cas_file;
use crate::routes::err_resp;
use crate::server::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use parking_lot::Mutex as SyncMutex;
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use uuid::Uuid;

pub const DEFAULT_MANIFEST_UPSTREAM_URL: &str =
    "https://github.com/bhrumom/gpui-telegram/releases/latest/download/latest-mac.json";
pub const DEFAULT_ASSET_UPSTREAM_BASE: &str =
    "https://github.com/bhrumom/gpui-telegram/releases/download";
const MANIFEST_CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_CACHED_TAG_DIRS: usize = 2;

/// In-memory manifest cache and single-flight download lock table shared across relay requests.
#[derive(Debug, Default)]
pub struct UpdateRelayState {
    manifest_cache: RwLock<Option<(Instant, String)>>,
    fetch_locks: SyncMutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl UpdateRelayState {
    pub fn new() -> Self {
        Self::default()
    }

    fn acquire_fetch_lock(&self, key: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self.fetch_locks.lock();
        if map.len() > 64 {
            map.retain(|_, weak| weak.strong_count() > 0);
        }
        if let Some(weak) = map.get(key) {
            if let Some(strong) = weak.upgrade() {
                return strong;
            }
        }
        let lock = Arc::new(AsyncMutex::new(()));
        map.insert(key.to_string(), Arc::downgrade(&lock));
        lock
    }
}

fn is_valid_release_tag(tag: &str) -> bool {
    if tag.is_empty() || tag.len() > 64 || tag.contains("..") {
        return false;
    }
    if !tag
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return false;
    }
    tag.starts_with('v')
        || tag.starts_with('V')
        || tag.starts_with("macos-main-")
        || tag.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

fn is_valid_release_filename(filename: &str) -> bool {
    if filename.is_empty()
        || filename.len() > 128
        || filename.contains("..")
        || filename.contains('/')
        || filename.contains('\\')
    {
        return false;
    }
    if !filename
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return false;
    }
    filename.starts_with("Fabushi-")
        || filename.starts_with("gpui_telegram")
        || filename.starts_with("gpui-tg-cli")
        || filename == "latest-mac.json"
        || filename == "latest-windows.json"
        || filename == "SHA256SUMS"
        || filename == "SHA256SUMS-windows.txt"
}

fn resolve_updates_cache_root(state: &AppState) -> PathBuf {
    if state.config.is_in_memory() {
        state.cas.base_dir().join("updates_cache")
    } else {
        let db_path = PathBuf::from(state.config.resolve_db_path());
        let parent = db_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| FsPath::new("."));
        parent.join("updates_cache")
    }
}

fn build_relay_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("Fabushi-Update-Relay/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(45))
        .tcp_keepalive(Duration::from_secs(15))
        .http1_only()
        .build()
        .map_err(|e| format!("Failed building update relay HTTP client: {e}"))
}

async fn prune_old_tag_caches(cache_root: &FsPath, keep_tag: &str) {
    let Ok(mut entries) = fs::read_dir(cache_root).await else {
        return;
    };
    let mut dirs: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        dirs.push((modified, name, path));
    }
    if dirs.len() <= MAX_CACHED_TAG_DIRS {
        return;
    }
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut kept = 0usize;
    for (_, name, path) in dirs {
        if name == keep_tag || kept < MAX_CACHED_TAG_DIRS {
            kept += 1;
            continue;
        }
        let _ = fs::remove_dir_all(path).await;
    }
}

pub async fn updates_latest_manifest(State(state): State<Arc<AppState>>) -> Response {
    serve_latest_manifest(&state, DEFAULT_MANIFEST_UPSTREAM_URL).await
}

pub async fn updates_download_asset(
    State(state): State<Arc<AppState>>,
    Path((tag, filename)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    serve_download_asset(&state, &tag, &filename, &headers, DEFAULT_ASSET_UPSTREAM_BASE).await
}

pub(crate) async fn serve_latest_manifest(state: &AppState, upstream_url: &str) -> Response {
    {
        let guard = state.update_relay.manifest_cache.read().await;
        if let Some((fetched_at, ref body)) = *guard {
            if fetched_at.elapsed() < MANIFEST_CACHE_TTL {
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                        (header::CACHE_CONTROL, "public, max-age=60"),
                    ],
                    body.clone(),
                )
                    .into_response();
            }
        }
    }

    let client = match build_relay_client() {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "relay_client_error", &e),
    };
    let resp = match client.get(upstream_url).timeout(Duration::from_secs(15)).send().await {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_GATEWAY,
                "upstream_manifest_failed",
                &format!("Failed fetching release manifest: {e}"),
            );
        }
    };
    if !resp.status().is_success() {
        return err_resp(
            StatusCode::BAD_GATEWAY,
            "upstream_manifest_http",
            &format!("Upstream manifest returned HTTP {}", resp.status()),
        );
    }
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_GATEWAY,
                "upstream_manifest_read",
                &format!("Failed reading release manifest: {e}"),
            );
        }
    };
    if serde_json::from_str::<serde_json::Value>(&body).is_err() {
        return err_resp(
            StatusCode::BAD_GATEWAY,
            "upstream_manifest_invalid",
            "Upstream release manifest is not valid JSON",
        );
    }

    {
        let mut guard = state.update_relay.manifest_cache.write().await;
        *guard = Some((Instant::now(), body.clone()));
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body,
    )
        .into_response()
}

pub(crate) async fn serve_download_asset(
    state: &AppState,
    tag: &str,
    filename: &str,
    headers: &HeaderMap,
    upstream_base: &str,
) -> Response {
    if !is_valid_release_tag(tag) || !is_valid_release_filename(filename) {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_update_asset",
            "Invalid update release tag or filename",
        );
    }

    let cache_root = resolve_updates_cache_root(state);
    let tag_dir = cache_root.join(tag);
    let cached_path = tag_dir.join(filename);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    if fs::metadata(&cached_path).await.is_ok_and(|m| m.is_file() && m.len() > 0) {
        return stream_cas_file(&cached_path, range_header, Some("application/octet-stream")).await;
    }

    let lock = state
        .update_relay
        .acquire_fetch_lock(&format!("{tag}/{filename}"));
    let _guard = lock.lock().await;

    if !fs::metadata(&cached_path).await.is_ok_and(|m| m.is_file() && m.len() > 0) {
        if let Err(e) = fs::create_dir_all(&tag_dir).await {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cache_dir_error",
                &format!("Failed creating update cache directory: {e}"),
            );
        }

        let client = match build_relay_client() {
            Ok(c) => c,
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "relay_client_error", &e),
        };
        let upstream_url = format!("{}/{tag}/{filename}", upstream_base.trim_end_matches('/'));
        let resp = match client
            .get(&upstream_url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_GATEWAY,
                    "upstream_asset_failed",
                    &format!("Failed fetching release asset from GitHub: {e}"),
                );
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return err_resp(
                StatusCode::NOT_FOUND,
                "update_asset_not_found",
                &format!("Release asset {tag}/{filename} not found upstream"),
            );
        }
        if !resp.status().is_success() {
            return err_resp(
                StatusCode::BAD_GATEWAY,
                "upstream_asset_http",
                &format!("Upstream release asset returned HTTP {}", resp.status()),
            );
        }

        let expected_len = resp.content_length();
        let tmp_path = tag_dir.join(format!("{filename}.tmp.{}", Uuid::new_v4().simple()));
        let mut file = match fs::File::create(&tmp_path).await {
            Ok(f) => f,
            Err(e) => {
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cache_write_error",
                    &format!("Failed creating update cache temp file: {e}"),
                );
            }
        };

        let mut written: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk_res) = stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(e) => {
                    drop(file);
                    let _ = fs::remove_file(&tmp_path).await;
                    return err_resp(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_error",
                        &format!("Upstream release stream interrupted: {e}"),
                    );
                }
            };
            if let Err(e) = file.write_all(&chunk).await {
                drop(file);
                let _ = fs::remove_file(&tmp_path).await;
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cache_write_error",
                    &format!("Failed writing update cache file: {e}"),
                );
            }
            written = written.saturating_add(chunk.len() as u64);
        }

        if let Some(exp) = expected_len {
            if written < exp {
                drop(file);
                let _ = fs::remove_file(&tmp_path).await;
                return err_resp(
                    StatusCode::BAD_GATEWAY,
                    "upstream_truncated",
                    &format!("Upstream asset truncated ({written}/{exp} bytes)"),
                );
            }
        }

        if let Err(e) = file.sync_all().await {
            drop(file);
            let _ = fs::remove_file(&tmp_path).await;
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cache_sync_error",
                &format!("Failed syncing update cache file: {e}"),
            );
        }
        drop(file);

        if let Err(e) = fs::rename(&tmp_path, &cached_path).await {
            let _ = fs::remove_file(&tmp_path).await;
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cache_promote_error",
                &format!("Failed committing update cache file: {e}"),
            );
        }
        prune_old_tag_caches(&cache_root, tag).await;
    }

    stream_cas_file(&cached_path, range_header, Some("application/octet-stream")).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use axum::body::to_bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_updates_allowlist_validation() {
        assert!(is_valid_release_tag("v1.3.0"));
        assert!(is_valid_release_tag("macos-main-v1.2.78"));
        assert!(!is_valid_release_tag(".."));
        assert!(!is_valid_release_tag("v1.3.0/../v1.2.0"));
        assert!(!is_valid_release_tag("bad/tag"));
        assert!(!is_valid_release_tag("evil"));

        assert!(is_valid_release_filename("Fabushi-1.3.0-macos-arm64.zip"));
        assert!(is_valid_release_filename("gpui_telegram-v1.3.0-linux-x86_64.tar.gz"));
        assert!(is_valid_release_filename("latest-mac.json"));
        assert!(!is_valid_release_filename("../etc/passwd"));
        assert!(!is_valid_release_filename("secret.db"));
        assert!(!is_valid_release_filename("Fabushi-1.3.0.zip?foo=bar"));
    }

    #[tokio::test]
    async fn test_updates_relay_rejects_invalid_and_serves_cached_range() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ServerConfig {
            database_url: ":memory:".to_string(),
            media_storage_path: tmp.path().join("cas").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = Arc::new(AppState::new(cfg));

        let bad_resp = serve_download_asset(
            &state,
            "v1.3.0",
            "unauthorized.bin",
            &HeaderMap::new(),
            "http://127.0.0.1:1",
        )
        .await;
        assert_eq!(bad_resp.status(), StatusCode::BAD_REQUEST);

        let bad_tag = serve_download_asset(
            &state,
            "../etc",
            "Fabushi-1.3.0-macos-arm64.zip",
            &HeaderMap::new(),
            "http://127.0.0.1:1",
        )
        .await;
        assert_eq!(bad_tag.status(), StatusCode::BAD_REQUEST);

        let payload = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".to_vec();
        let payload_len = payload.len();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req_buf = [0u8; 1024];
            let _ = sock.read(&mut req_buf).await.unwrap();
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {payload_len}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
            );
            sock.write_all(hdr.as_bytes()).await.unwrap();
            sock.write_all(&payload).await.unwrap();
        });

        let upstream_base = format!("http://{addr}");
        let full_resp = serve_download_asset(
            &state,
            "v1.3.0",
            "Fabushi-1.3.0-macos-arm64.zip",
            &HeaderMap::new(),
            &upstream_base,
        )
        .await;
        assert_eq!(full_resp.status(), StatusCode::OK);
        let full_bytes = to_bytes(full_resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&full_bytes[..], b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz");
        server_task.await.unwrap();

        // Subsequent 206 Range request must be served from disk cache without hitting upstream again.
        let mut range_headers = HeaderMap::new();
        range_headers.insert(header::RANGE, "bytes=10-19".parse().unwrap());
        let range_resp = serve_download_asset(
            &state,
            "v1.3.0",
            "Fabushi-1.3.0-macos-arm64.zip",
            &range_headers,
            "http://127.0.0.1:1",
        )
        .await;
        assert_eq!(range_resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            range_resp.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 10-19/62"
        );
        let range_bytes = to_bytes(range_resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&range_bytes[..], b"ABCDEFGHIJ");
    }
}
