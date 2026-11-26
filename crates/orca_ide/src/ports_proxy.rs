//! OS Listening-Port Scanner (`/proc/net/tcp` + `/proc/net/tcp6` + `/proc/<pid>/fd`)
//! and Pure-Rust `*.orca.localhost` HTTP/1.1 + RFC 6455 WebSocket Reverse Proxy (`F10`).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use bytes::BytesMut;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{OrcaIdeRuntimeError, Result};

/// RFC 6455 §1.3 WebSocket handshake GUID.
pub const WEBSOCKET_ACCEPT_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Hostname suffix required by `LocalhostWorktreeLabelProxy`.
pub const ORCA_LOCALHOST_SUFFIX: &str = ".orca.localhost";

const MAX_HTTP_HEAD_BYTES: usize = 64 * 1024;
const PROXY_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Raw listening socket parsed from Linux `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcNetSocket {
    pub local_ip: IpAddr,
    pub port: u16,
    pub state: u8,
    pub inode: u64,
    pub is_ipv6: bool,
}

/// Listening socket enriched with optional OS process & worktree attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningSocketEntry {
    pub local_ip: IpAddr,
    pub port: u16,
    pub inode: u64,
    pub is_ipv6: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_label: Option<String>,
}

/// Attributed dev-server port mapped to a worktree label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreePortAttribution {
    pub worktree_label: String,
    pub worktree_path: PathBuf,
    pub port: u16,
    pub local_ip: IpAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
}

/// Parses Linux `/proc/net/tcp` (`is_ipv6 = false`) or `/proc/net/tcp6` (`is_ipv6 = true`),
/// returning only sockets in state `0x0A` (`TCP_LISTEN`).
pub fn parse_proc_net_tcp(contents: &str, is_ipv6: bool) -> Vec<ProcNetSocket> {
    let mut out = Vec::new();
    for line in contents.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        let Ok(state) = u8::from_str_radix(cols[3], 16) else {
            continue;
        };
        if state != 0x0A {
            continue;
        }
        let Some((ip_hex, port_hex)) = cols[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let Some(local_ip) = decode_proc_net_ip(ip_hex, is_ipv6) else {
            continue;
        };
        let Ok(inode) = cols[9].parse::<u64>() else {
            continue;
        };
        out.push(ProcNetSocket {
            local_ip,
            port,
            state,
            inode,
            is_ipv6,
        });
    }
    out
}

fn decode_proc_net_ip(hex_ip: &str, is_ipv6: bool) -> Option<IpAddr> {
    if !is_ipv6 {
        if hex_ip.len() != 8 {
            return None;
        }
        let raw = u32::from_str_radix(hex_ip, 16).ok()?;
        let octets = raw.to_le_bytes();
        Some(IpAddr::V4(Ipv4Addr::new(
            octets[0], octets[1], octets[2], octets[3],
        )))
    } else {
        if hex_ip.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for chunk_idx in 0..4 {
            let start = chunk_idx * 8;
            let word = u32::from_str_radix(&hex_ip[start..start + 8], 16).ok()?;
            let le = word.to_le_bytes();
            bytes[chunk_idx * 4..chunk_idx * 4 + 4].copy_from_slice(&le);
        }
        Some(IpAddr::V6(Ipv6Addr::from(bytes)))
    }
}

#[derive(Debug, Clone)]
struct ProcProcessInfo {
    pid: u32,
    process_name: Option<String>,
    cwd: Option<PathBuf>,
}

fn map_socket_inodes_to_processes(target_inodes: &[u64]) -> HashMap<u64, ProcProcessInfo> {
    let mut result = HashMap::new();
    if target_inodes.is_empty() {
        return result;
    }
    let Ok(proc_entries) = std::fs::read_dir("/proc") else {
        return result;
    };

    for proc_entry in proc_entries.flatten() {
        let fname = proc_entry.file_name();
        let Some(pid) = fname.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let fd_dir = proc_entry.path().join("fd");
        let Ok(fds) = std::fs::read_dir(&fd_dir) else {
            continue;
        };
        let mut matched_inodes = Vec::new();
        for fd_entry in fds.flatten() {
            if let Ok(link_target) = std::fs::read_link(fd_entry.path())
                && let Some(link_str) = link_target.to_str()
                && let Some(inner) = link_str
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix(']'))
                && let Ok(inode) = inner.parse::<u64>()
                && target_inodes.contains(&inode)
            {
                matched_inodes.push(inode);
            }
        }
        if !matched_inodes.is_empty() {
            let process_name = std::fs::read_to_string(proc_entry.path().join("comm"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let cwd = std::fs::read_link(proc_entry.path().join("cwd")).ok();
            let info = ProcProcessInfo {
                pid,
                process_name,
                cwd,
            };
            for inode in matched_inodes {
                result.insert(inode, info.clone());
            }
        }
    }
    result
}

/// Scans listening local dev-server TCP ports and attributes them to worktree labels.
#[derive(Debug, Clone, Default)]
pub struct ListeningPortScanner {
    mock_entries: Option<Vec<ListeningSocketEntry>>,
}

impl ListeningPortScanner {
    /// Creates a live scanner reading `/proc/net/tcp`, `/proc/net/tcp6`, and `/proc/<pid>/fd`.
    pub fn new() -> Self {
        Self { mock_entries: None }
    }

    /// Creates a deterministic scanner backed by an injectable listening-socket table.
    pub fn with_mock_entries(entries: Vec<ListeningSocketEntry>) -> Self {
        Self {
            mock_entries: Some(entries),
        }
    }

    /// Returns all currently listening TCP sockets (IPv4 + IPv6) with best-effort PID/CWD info.
    pub fn scan_listening_ports(&self) -> Vec<ListeningSocketEntry> {
        if let Some(mock) = &self.mock_entries {
            return mock.clone();
        }
        let mut raw = Vec::new();
        if let Ok(v4) = std::fs::read_to_string("/proc/net/tcp") {
            raw.extend(parse_proc_net_tcp(&v4, false));
        }
        if let Ok(v6) = std::fs::read_to_string("/proc/net/tcp6") {
            raw.extend(parse_proc_net_tcp(&v6, true));
        }
        let inodes: Vec<u64> = raw.iter().map(|s| s.inode).collect();
        let proc_map = map_socket_inodes_to_processes(&inodes);

        raw.into_iter()
            .map(|sock| {
                let info = proc_map.get(&sock.inode);
                ListeningSocketEntry {
                    local_ip: sock.local_ip,
                    port: sock.port,
                    inode: sock.inode,
                    is_ipv6: sock.is_ipv6,
                    pid: info.map(|i| i.pid),
                    process_name: info.and_then(|i| i.process_name.clone()),
                    cwd: info.and_then(|i| i.cwd.clone()),
                    worktree_label: None,
                }
            })
            .collect()
    }

    /// Attributes discovered listening ports to `(worktree_label, worktree_path)` entries
    /// using longest-prefix CWD matching (or pre-assigned `worktree_label`).
    pub fn attribute_ports_to_worktrees(
        &self,
        worktrees: &[(String, PathBuf)],
    ) -> Vec<WorktreePortAttribution> {
        let sockets = self.scan_listening_ports();
        let mut out = Vec::new();
        for sock in sockets {
            if let Some(label) = &sock.worktree_label {
                let wt_path = worktrees
                    .iter()
                    .find(|(l, _)| l == label)
                    .map(|(_, p)| p.clone())
                    .or_else(|| sock.cwd.clone())
                    .unwrap_or_default();
                out.push(WorktreePortAttribution {
                    worktree_label: label.clone(),
                    worktree_path: wt_path,
                    port: sock.port,
                    local_ip: sock.local_ip,
                    pid: sock.pid,
                    process_name: sock.process_name,
                });
                continue;
            }
            let Some(cwd) = &sock.cwd else {
                continue;
            };
            let best = worktrees
                .iter()
                .filter(|(_, wt_path)| path_starts_with_clean(cwd, wt_path))
                .max_by_key(|(_, wt_path)| wt_path.as_os_str().len());
            if let Some((label, wt_path)) = best {
                out.push(WorktreePortAttribution {
                    worktree_label: label.clone(),
                    worktree_path: wt_path.clone(),
                    port: sock.port,
                    local_ip: sock.local_ip,
                    pid: sock.pid,
                    process_name: sock.process_name,
                });
            }
        }
        out.sort_by(|a, b| {
            a.worktree_label
                .cmp(&b.worktree_label)
                .then_with(|| a.port.cmp(&b.port))
        });
        out
    }
}

fn path_starts_with_clean(candidate: &Path, root: &Path) -> bool {
    if candidate.starts_with(root) {
        return true;
    }
    if let (Ok(c_canon), Ok(r_canon)) = (candidate.canonicalize(), root.canonicalize()) {
        return c_canon.starts_with(r_canon);
    }
    false
}

/// Computes the exact RFC 6455 `Sec-WebSocket-Accept` header value from `Sec-WebSocket-Key`:
/// `base64(SHA-1(key.trim() || "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))`.
pub fn compute_websocket_accept(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.trim().as_bytes());
    hasher.update(WEBSOCKET_ACCEPT_GUID.as_bytes());
    let digest = hasher.finalize();
    BASE64_STANDARD.encode(digest)
}

/// Host resolution failure mapped to HTTP `403 Forbidden` or `404 Not Found`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyHostError {
    ForbiddenHost(String),
    UnknownLabel(String),
}

fn validate_worktree_label(raw: &str) -> Result<String> {
    let label = raw.trim().to_ascii_lowercase();
    if label.is_empty()
        || label.len() > 63
        || label.starts_with('-')
        || label.ends_with('-')
        || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "invalid worktree label '{raw}': must be 1..=63 DNS label chars [a-z0-9-]"
        )));
    }
    Ok(label)
}

/// Pure-Rust `*.orca.localhost` HTTP/1.1 + RFC 6455 WebSocket reverse proxy (`Send + Sync`).
pub struct LocalhostWorktreeLabelProxy {
    local_addr: SocketAddr,
    routes: Arc<RwLock<BTreeMap<String, u16>>>,
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl LocalhostWorktreeLabelProxy {
    /// Binds a live loopback TCP listener on `127.0.0.1:0` and begins proxying traffic.
    pub async fn bind_loopback() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let routes = Arc::new(RwLock::new(BTreeMap::new()));
        let cancel = CancellationToken::new();

        let accept_routes = Arc::clone(&routes);
        let accept_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = accept_cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _peer)) => {
                                let conn_routes = Arc::clone(&accept_routes);
                                let conn_cancel = accept_cancel.clone();
                                tokio::spawn(async move {
                                    let _ = handle_proxy_connection(stream, conn_routes, conn_cancel).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            routes,
            cancel,
            task: Mutex::new(Some(handle)),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn register_route(&self, worktree_label: &str, target_port: u16) -> Result<String> {
        if target_port == 0 {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "target_port must be non-zero",
            ));
        }
        let label = validate_worktree_label(worktree_label)?;
        self.routes.write().insert(label.clone(), target_port);
        Ok(label)
    }

    pub fn unregister_route(&self, worktree_label: &str) -> Option<u16> {
        let label = worktree_label.trim().to_ascii_lowercase();
        self.routes.write().remove(&label)
    }

    pub fn list_routes(&self) -> BTreeMap<String, u16> {
        self.routes.read().clone()
    }

    pub fn resolve_host(
        &self,
        host_header: &str,
    ) -> std::result::Result<(String, u16), ProxyHostError> {
        resolve_host_against_table(host_header, &self.routes.read())
    }

    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let handle = self.task.lock().take();
        if let Some(h) = handle {
            let _ = h.await;
        }
    }
}

impl Drop for LocalhostWorktreeLabelProxy {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn resolve_host_against_table(
    host_header: &str,
    routes: &BTreeMap<String, u16>,
) -> std::result::Result<(String, u16), ProxyHostError> {
    let trimmed = host_header.trim();
    if trimmed.is_empty() {
        return Err(ProxyHostError::ForbiddenHost(String::new()));
    }
    let host_only = if let Some(stripped) = trimmed.strip_prefix('[') {
        // Bracketed IPv6 literal cannot be `*.orca.localhost`
        return Err(ProxyHostError::ForbiddenHost(stripped.to_string()));
    } else {
        trimmed
            .rsplit_once(':')
            .map_or(trimmed, |(h, _port)| h)
            .to_ascii_lowercase()
    };

    let Some(label) = host_only.strip_suffix(ORCA_LOCALHOST_SUFFIX) else {
        return Err(ProxyHostError::ForbiddenHost(host_only));
    };
    if label.is_empty()
        || label.contains('.')
        || label.starts_with('-')
        || label.ends_with('-')
        || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ProxyHostError::ForbiddenHost(host_only));
    }

    match routes.get(label) {
        Some(&port) => Ok((label.to_string(), port)),
        None => Err(ProxyHostError::UnknownLabel(label.to_string())),
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

async fn read_http_head(stream: &mut TcpStream) -> std::io::Result<(Vec<u8>, BytesMut)> {
    let mut buf = BytesMut::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        if let Some(end) = find_header_end(&buf) {
            let head = buf.split_to(end).to_vec();
            return Ok((head, buf));
        }
        if buf.len() >= MAX_HTTP_HEAD_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP header section exceeded max size",
            ));
        }
        let n = tokio::time::timeout(PROXY_IO_TIMEOUT, stream.read(&mut tmp))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "head read timeout"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "EOF before HTTP headers completed",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

struct ParsedHttpHead {
    start_line: String,
    headers: Vec<(String, String)>,
}

fn parse_http_head(raw: &[u8]) -> std::result::Result<ParsedHttpHead, &'static str> {
    let text = std::str::from_utf8(raw).map_err(|_| "non-UTF8 HTTP head")?;
    let mut lines = text.split("\r\n");
    let start_line = lines.next().ok_or("missing HTTP start line")?.to_string();
    if start_line.is_empty() {
        return Err("empty HTTP start line");
    }
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("malformed HTTP header line");
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Ok(ParsedHttpHead {
        start_line,
        headers,
    })
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn is_websocket_upgrade(headers: &[(String, String)]) -> Option<String> {
    let upgrade = find_header(headers, "upgrade")?;
    if !upgrade.eq_ignore_ascii_case("websocket") {
        return None;
    }
    let connection = find_header(headers, "connection")?;
    let has_upgrade_token = connection
        .split(',')
        .any(|tok| tok.trim().eq_ignore_ascii_case("upgrade"));
    if !has_upgrade_token {
        return None;
    }
    let ws_key = find_header(headers, "sec-websocket-key")?.trim();
    if ws_key.is_empty() {
        return None;
    }
    Some(ws_key.to_string())
}

async fn write_simple_http_error(
    stream: &mut TcpStream,
    status_code: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {status_code} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

fn build_forwarded_request_head(
    parsed: &ParsedHttpHead,
    original_host: &str,
    worktree_label: &str,
    is_ws: bool,
) -> Vec<u8> {
    let mut out = String::with_capacity(512);
    out.push_str(&parsed.start_line);
    out.push_str("\r\n");
    for (name, value) in &parsed.headers {
        if !is_ws && name.eq_ignore_ascii_case("connection") {
            continue;
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.push_str("X-Forwarded-Host: ");
    out.push_str(original_host);
    out.push_str("\r\n");
    out.push_str("X-Orca-Worktree-Label: ");
    out.push_str(worktree_label);
    out.push_str("\r\n");
    if !is_ws {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    out.into_bytes()
}

async fn handle_proxy_connection(
    mut client: TcpStream,
    routes: Arc<RwLock<BTreeMap<String, u16>>>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let (raw_head, leftover_body) = match read_http_head(&mut client).await {
        Ok(pair) => pair,
        Err(_) => return Ok(()),
    };
    let parsed = match parse_http_head(&raw_head) {
        Ok(p) => p,
        Err(msg) => {
            let _ = write_simple_http_error(&mut client, 400, "Bad Request", msg).await;
            return Ok(());
        }
    };

    let host_header = find_header(&parsed.headers, "host").unwrap_or("");
    let resolved = {
        let table = routes.read();
        resolve_host_against_table(host_header, &table)
    };
    let (label, target_port) = match resolved {
        Ok(resolved) => resolved,
        Err(ProxyHostError::ForbiddenHost(host)) => {
            let _ = write_simple_http_error(
                &mut client,
                403,
                "Forbidden",
                &format!("host '{host}' is not an allowed *.orca.localhost origin"),
            )
            .await;
            return Ok(());
        }
        Err(ProxyHostError::UnknownLabel(unknown)) => {
            let _ = write_simple_http_error(
                &mut client,
                404,
                "Not Found",
                &format!("no listening route registered for worktree label '{unknown}'"),
            )
            .await;
            return Ok(());
        }
    };

    let mut upstream = match TcpStream::connect(("127.0.0.1", target_port)).await {
        Ok(s) => s,
        Err(err) => {
            let _ = write_simple_http_error(
                &mut client,
                502,
                "Bad Gateway",
                &format!("failed to connect to 127.0.0.1:{target_port}: {err}"),
            )
            .await;
            return Ok(());
        }
    };

    if let Some(ws_key) = is_websocket_upgrade(&parsed.headers) {
        let forwarded = build_forwarded_request_head(&parsed, host_header, &label, true);
        upstream.write_all(&forwarded).await?;
        if !leftover_body.is_empty() {
            upstream.write_all(&leftover_body).await?;
        }
        upstream.flush().await?;

        let (resp_head, upstream_leftover) = read_http_head(&mut upstream).await?;
        let parsed_resp = match parse_http_head(&resp_head) {
            Ok(p) => p,
            Err(_) => {
                let _ = write_simple_http_error(
                    &mut client,
                    502,
                    "Bad Gateway",
                    "malformed upstream WebSocket handshake response",
                )
                .await;
                return Ok(());
            }
        };

        let is_101 = parsed_resp
            .start_line
            .split_whitespace()
            .nth(1)
            .is_some_and(|code| code == "101");

        if is_101 {
            let expected_accept = compute_websocket_accept(&ws_key);
            if let Some(actual_accept) = find_header(&parsed_resp.headers, "sec-websocket-accept")
                && actual_accept.trim() != expected_accept
            {
                let _ = write_simple_http_error(
                    &mut client,
                    502,
                    "Bad Gateway",
                    "upstream Sec-WebSocket-Accept mismatch",
                )
                .await;
                return Ok(());
            }
            client.write_all(&resp_head).await?;
            if !upstream_leftover.is_empty() {
                client.write_all(&upstream_leftover).await?;
            }
            client.flush().await?;

            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
            }
            return Ok(());
        }

        // Non-101 response to upgrade request: relay status/headers/body and close.
        client.write_all(&resp_head).await?;
        if !upstream_leftover.is_empty() {
            client.write_all(&upstream_leftover).await?;
        }
        let _ = tokio::io::copy(&mut upstream, &mut client).await;
        return Ok(());
    }

    // Standard HTTP/1.1 request/response proxying (preserving Cookie & Set-Cookie).
    let forwarded = build_forwarded_request_head(&parsed, host_header, &label, false);
    upstream.write_all(&forwarded).await?;
    if !leftover_body.is_empty() {
        upstream.write_all(&leftover_body).await?;
    }
    if let Some(cl_str) = find_header(&parsed.headers, "content-length")
        && let Ok(content_len) = cl_str.parse::<usize>()
    {
        let remaining = content_len.saturating_sub(leftover_body.len());
        if remaining > 0 {
            let mut limited = (&mut client).take(remaining as u64);
            let _ = tokio::io::copy(&mut limited, &mut upstream).await;
        }
    }
    upstream.flush().await?;

    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = tokio::io::copy(&mut upstream, &mut client) => {}
    }
    let _ = client.flush().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn proc_net_tcp_and_tcp6_hex_parser_and_worktree_attribution() {
        assert_send_sync::<LocalhostWorktreeLabelProxy>();
        assert_send_sync::<ListeningPortScanner>();

        let tcp_v4 = concat!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
            "   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0\n",
            "   1: 0100007F:1F90 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 99999 1 0\n",
        );
        let parsed_v4 = parse_proc_net_tcp(tcp_v4, false);
        assert_eq!(parsed_v4.len(), 1);
        assert_eq!(parsed_v4[0].local_ip, IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(parsed_v4[0].port, 3000);
        assert_eq!(parsed_v4[0].inode, 12345);

        let tcp_v6 = concat!(
            "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
            "   0: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 54321 1 0\n",
        );
        let parsed_v6 = parse_proc_net_tcp(tcp_v6, true);
        assert_eq!(parsed_v6.len(), 1);
        assert_eq!(parsed_v6[0].local_ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(parsed_v6[0].port, 5173);

        let scanner = ListeningPortScanner::with_mock_entries(vec![ListeningSocketEntry {
            local_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 3000,
            inode: 12345,
            is_ipv6: false,
            pid: Some(4242),
            process_name: Some("vite".into()),
            cwd: Some(PathBuf::from("/worktrees/swift-otter/apps/web")),
            worktree_label: None,
        }]);
        let attributed = scanner.attribute_ports_to_worktrees(&[
            ("main".into(), PathBuf::from("/worktrees")),
            ("swift-otter".into(), PathBuf::from("/worktrees/swift-otter")),
        ]);
        assert_eq!(attributed.len(), 1);
        assert_eq!(attributed[0].worktree_label, "swift-otter");
        assert_eq!(attributed[0].port, 3000);
    }

    #[test]
    fn websocket_accept_matches_rfc6455_vector() {
        assert_eq!(
            compute_websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    async fn spawn_echo_http_and_ws_upstream() -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind upstream");
        let port = listener.local_addr().expect("addr").port();
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let Ok((head, _leftover)) = read_http_head(&mut stream).await else {
                        return;
                    };
                    let Ok(parsed) = parse_http_head(&head) else {
                        return;
                    };
                    if let Some(ws_key) = is_websocket_upgrade(&parsed.headers) {
                        let accept = compute_websocket_accept(&ws_key);
                        let resp = format!(
                            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.flush().await;
                        let mut buf = [0u8; 256];
                        if let Ok(n) = stream.read(&mut buf).await
                            && n > 0
                        {
                            let mut reply = b"echo:".to_vec();
                            reply.extend_from_slice(&buf[..n]);
                            let _ = stream.write_all(&reply).await;
                            let _ = stream.flush().await;
                        }
                    } else {
                        let cookie = find_header(&parsed.headers, "cookie").unwrap_or("");
                        let label = find_header(&parsed.headers, "x-orca-worktree-label")
                            .unwrap_or("");
                        let body = format!("label={label};cookie={cookie};line={}", parsed.start_line);
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nSet-Cookie: orca_sid=wt_{label}; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.flush().await;
                    }
                });
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn live_loopback_proxy_routes_http_and_rfc6455_websocket_and_enforces_host_gates() {
        let (upstream_port, upstream_task) = spawn_echo_http_and_ws_upstream().await;
        let proxy = LocalhostWorktreeLabelProxy::bind_loopback()
            .await
            .expect("bind proxy");
        proxy
            .register_route("swift-otter", upstream_port)
            .expect("register route");
        assert_eq!(proxy.list_routes().get("swift-otter"), Some(&upstream_port));

        // 1. Foreign host -> 403 Forbidden
        let mut s_403 = TcpStream::connect(proxy.local_addr()).await.expect("connect");
        s_403
            .write_all(b"GET / HTTP/1.1\r\nHost: evil.example.com\r\n\r\n")
            .await
            .expect("write");
        let mut resp_403 = String::new();
        s_403.read_to_string(&mut resp_403).await.expect("read 403");
        assert!(resp_403.starts_with("HTTP/1.1 403 Forbidden"), "{resp_403}");

        // 2. Unknown label -> 404 Not Found
        let mut s_404 = TcpStream::connect(proxy.local_addr()).await.expect("connect");
        s_404
            .write_all(b"GET / HTTP/1.1\r\nHost: missing-wt.orca.localhost\r\n\r\n")
            .await
            .expect("write");
        let mut resp_404 = String::new();
        s_404.read_to_string(&mut resp_404).await.expect("read 404");
        assert!(resp_404.starts_with("HTTP/1.1 404 Not Found"), "{resp_404}");

        // 3. Valid HTTP/1.1 proxying preserving Cookie & Set-Cookie
        let mut s_http = TcpStream::connect(proxy.local_addr()).await.expect("connect");
        let http_req = format!(
            "GET /api/ping HTTP/1.1\r\nHost: swift-otter.orca.localhost:{}\r\nCookie: tok=abc\r\n\r\n",
            proxy.port()
        );
        s_http.write_all(http_req.as_bytes()).await.expect("write");
        let mut resp_http = String::new();
        s_http.read_to_string(&mut resp_http).await.expect("read 200");
        assert!(resp_http.starts_with("HTTP/1.1 200 OK"), "{resp_http}");
        assert!(resp_http.contains("Set-Cookie: orca_sid=wt_swift-otter; Path=/"));
        assert!(resp_http.contains("label=swift-otter;cookie=tok=abc;line=GET /api/ping HTTP/1.1"));

        // 4. Valid RFC 6455 WebSocket 101 Upgrade + bidirectional tunnel streaming
        let mut s_ws = TcpStream::connect(proxy.local_addr()).await.expect("connect");
        let ws_req = concat!(
            "GET /ws HTTP/1.1\r\n",
            "Host: swift-otter.orca.localhost\r\n",
            "Upgrade: websocket\r\n",
            "Connection: Upgrade\r\n",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
            "Sec-WebSocket-Version: 13\r\n\r\n"
        );
        s_ws.write_all(ws_req.as_bytes()).await.expect("write ws");
        let (ws_head, _left) = read_http_head(&mut s_ws).await.expect("read 101");
        let ws_head_str = std::str::from_utf8(&ws_head).expect("utf8");
        assert!(ws_head_str.starts_with("HTTP/1.1 101 Switching Protocols"), "{ws_head_str}");
        assert!(ws_head_str.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));

        s_ws.write_all(b"ping-over-ws").await.expect("send ws payload");
        s_ws.flush().await.expect("flush");
        let mut echoed = [0u8; 64];
        let n = s_ws.read(&mut echoed).await.expect("read ws echo");
        assert_eq!(&echoed[..n], b"echo:ping-over-ws");

        proxy.shutdown().await;
        upstream_task.abort();
    }
}
