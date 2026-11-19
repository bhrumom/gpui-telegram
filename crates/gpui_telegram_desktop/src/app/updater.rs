//! Native Fabushi Desktop auto-updater & one-click self-installer.
//!
//! Checks `bhrumom/gpui-telegram` GitHub Releases (primary `latest-mac.json` manifest
//! with automatic fallback to GitHub Releases API `/releases/latest`), downloads and
//! SHA-256 verifies `.zip`/`.tar.gz` update archives, unpacks the staged `.app` bundle,
//! and atomically replaces the installed application bundle before relaunching.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use futures::StreamExt;
use gpui_telegram_client::net::proxy::ClientProxyConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_GITHUB_REPO: &str = "bhrumom/gpui-telegram";
#[cfg(target_os = "windows")]
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/bhrumom/gpui-telegram/releases/latest/download/latest-windows.json";
#[cfg(not(target_os = "windows"))]
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/bhrumom/gpui-telegram/releases/latest/download/latest-mac.json";
pub const DEFAULT_GITHUB_LATEST_API_URL: &str =
    "https://api.github.com/repos/bhrumom/gpui-telegram/releases/latest";
pub const CURRENT_APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Network, proxy, relay, and stall-retry tuning options for [`DesktopUpdater`].
#[derive(Debug, Clone, Default)]
pub struct UpdaterNetworkOptions {
    pub proxy: Option<ClientProxyConfig>,
    pub server_url: Option<String>,
    pub connect_timeout: Option<Duration>,
    pub chunk_idle_timeout: Option<Duration>,
    pub max_stall_retries: Option<usize>,
    #[doc(hidden)]
    pub release_download_base_override: Option<String>,
}

/// Formats human-readable download progress text including MB transferred when total size is known.
pub(crate) fn format_download_progress_text(
    version: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    progress_pct: u8,
    is_chinese: bool,
) -> String {
    if downloaded_bytes > 0 {
        if let Some(total) = total_bytes.filter(|&t| t > 0) {
            let dl_mb = downloaded_bytes as f64 / (1024.0 * 1024.0);
            let total_mb = total as f64 / (1024.0 * 1024.0);
            return if is_chinese {
                format!(
                    "正在下载 Fabushi v{version} ({progress_pct}% · {dl_mb:.1}/{total_mb:.1} MB)…"
                )
            } else {
                format!(
                    "Downloading Fabushi v{version} ({progress_pct}% · {dl_mb:.1}/{total_mb:.1} MB)…"
                )
            };
        }
    }
    if is_chinese {
        format!("正在下载 Fabushi v{version} ({progress_pct}%)…")
    } else {
        format!("Downloading Fabushi v{version} ({progress_pct}%)…")
    }
}

/// Metadata describing an installable update release discovered by the updater.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableUpdateInfo {
    pub version: String,
    pub asset_url: String,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
    pub release_notes: Option<String>,
    pub dmg_url: Option<String>,
    pub pkg_url: Option<String>,
}

/// Per-platform downloadable artifact entry inside `latest-mac.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAssetEntry {
    pub zip_url: String,
    #[serde(default)]
    pub zip_sha256: Option<String>,
    #[serde(default)]
    pub zip_size: Option<u64>,
    #[serde(default)]
    pub dmg_url: Option<String>,
    #[serde(default)]
    pub pkg_url: Option<String>,
}

/// Primary release manifest JSON (`latest-mac.json`) published to GitHub Releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateReleaseManifest {
    pub version: String,
    #[serde(default)]
    pub tag_name: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    pub platforms: HashMap<String, PlatformAssetEntry>,
}

/// Observable state machine for in-app update discovery, download, staging, and restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopUpdateState {
    Idle {
        last_checked_ok: bool,
        last_error: Option<String>,
    },
    Checking,
    UpToDate {
        checked_version: String,
    },
    Available {
        info: AvailableUpdateInfo,
    },
    Downloading {
        info: AvailableUpdateInfo,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        progress_pct: u8,
    },
    Installing {
        version: String,
    },
    ReadyToRestart {
        version: String,
        staged_app_path: PathBuf,
    },
    Failed {
        error: String,
        retry_info: Option<AvailableUpdateInfo>,
    },
}

impl Default for DesktopUpdateState {
    fn default() -> Self {
        Self::Idle {
            last_checked_ok: false,
            last_error: None,
        }
    }
}

/// Snapshot projected into settings views and menu drawers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopUpdateSnapshot {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub status_text: String,
    pub progress_pct: Option<u8>,
    pub is_busy: bool,
}

impl DesktopUpdateState {
    /// Returns update metadata when a downloadable or retryable release is known.
    pub fn actionable_info(&self) -> Option<&AvailableUpdateInfo> {
        match self {
            Self::Available { info } | Self::Downloading { info, .. } => Some(info),
            Self::Failed { retry_info, .. } => retry_info.as_ref(),
            _ => None,
        }
    }

    /// Formats a localized status projection for Settings and menu indicators.
    pub fn to_snapshot(&self, is_chinese: bool) -> DesktopUpdateSnapshot {
        let current = CURRENT_APP_VERSION.to_string();
        match self {
            Self::Idle {
                last_checked_ok,
                last_error,
            } => {
                let status_text = if let Some(err) = last_error {
                    if is_chinese {
                        format!("上次检查失败: {err}")
                    } else {
                        format!("Last check failed: {err}")
                    }
                } else if *last_checked_ok {
                    if is_chinese {
                        format!("已是最新版本 (v{current})")
                    } else {
                        format!("Up to date (v{current})")
                    }
                } else if is_chinese {
                    format!("当前版本 Fabushi v{current}")
                } else {
                    format!("Current version Fabushi v{current}")
                };
                DesktopUpdateSnapshot {
                    current_version: current,
                    latest_version: None,
                    status_text,
                    progress_pct: None,
                    is_busy: false,
                }
            }
            Self::Checking => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: None,
                status_text: if is_chinese {
                    "正在检查更新…".to_string()
                } else {
                    "Checking for updates…".to_string()
                },
                progress_pct: None,
                is_busy: true,
            },
            Self::UpToDate { checked_version } => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: None,
                status_text: if is_chinese {
                    format!("当前已是最新版本 (v{checked_version})")
                } else {
                    format!("You're up to date (v{checked_version})")
                },
                progress_pct: None,
                is_busy: false,
            },
            Self::Available { info } => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: Some(info.version.clone()),
                status_text: if is_chinese {
                    format!("发现新版本 Fabushi v{}，可点击一键更新", info.version)
                } else {
                    format!("Fabushi v{} is available for one-click update", info.version)
                },
                progress_pct: None,
                is_busy: false,
            },
            Self::Downloading {
                info,
                downloaded_bytes,
                total_bytes,
                progress_pct,
            } => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: Some(info.version.clone()),
                status_text: format_download_progress_text(
                    &info.version,
                    *downloaded_bytes,
                    *total_bytes,
                    *progress_pct,
                    is_chinese,
                ),
                progress_pct: Some(*progress_pct),
                is_busy: true,
            },
            Self::Installing { version } => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: Some(version.clone()),
                status_text: if is_chinese {
                    format!("正在安装 Fabushi v{version} 并准备重启…")
                } else {
                    format!("Installing Fabushi v{version} & restarting…")
                },
                progress_pct: Some(100),
                is_busy: true,
            },
            Self::ReadyToRestart { version, .. } => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: Some(version.clone()),
                status_text: if is_chinese {
                    format!("Fabushi v{version} 已就绪，正在重启…")
                } else {
                    format!("Fabushi v{version} is ready, restarting…")
                },
                progress_pct: Some(100),
                is_busy: true,
            },
            Self::Failed { error, retry_info } => DesktopUpdateSnapshot {
                current_version: current,
                latest_version: retry_info.as_ref().map(|i| i.version.clone()),
                status_text: if is_chinese {
                    format!("更新失败: {error}")
                } else {
                    format!("Update failed: {error}")
                },
                progress_pct: None,
                is_busy: false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSemver {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Option<Vec<PreSegment>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreSegment {
    Num(u64),
    Text(String),
}

/// Normalizes a version string by trimming whitespace and leading tag prefixes (`macos-main-v`, `v`, `V`).
pub fn normalize_version_string(raw: &str) -> String {
    let mut s = raw.trim();
    for prefix in ["macos-main-v", "macos-main-", "v", "V"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    if let Some((before_build, _)) = s.split_once('+') {
        s = before_build;
    }
    s.trim().to_string()
}

fn parse_semver(raw: &str) -> Option<ParsedSemver> {
    let norm = normalize_version_string(raw);
    if norm.is_empty() {
        return None;
    }
    let (core, pre_raw) = match norm.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (norm.as_str(), None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch_raw = parts.next().unwrap_or("0");
    let patch = patch_raw.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let prerelease = pre_raw.and_then(|p| {
        let trimmed = p.trim();
        if trimmed.is_empty() {
            return None;
        }
        let segs = trimmed
            .split('.')
            .map(|seg| match seg.parse::<u64>() {
                Ok(n) => PreSegment::Num(n),
                Err(_) => PreSegment::Text(seg.to_ascii_lowercase()),
            })
            .collect();
        Some(segs)
    });
    Some(ParsedSemver {
        major,
        minor,
        patch,
        prerelease,
    })
}

fn compare_prerelease(a: &[PreSegment], b: &[PreSegment]) -> Ordering {
    for (seg_a, seg_b) in a.iter().zip(b.iter()) {
        let ord = match (seg_a, seg_b) {
            (PreSegment::Num(x), PreSegment::Num(y)) => x.cmp(y),
            (PreSegment::Num(_), PreSegment::Text(_)) => Ordering::Less,
            (PreSegment::Text(_), PreSegment::Num(_)) => Ordering::Greater,
            (PreSegment::Text(x), PreSegment::Text(y)) => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

/// Compares two semantic version strings (stable > prerelease for identical major.minor.patch).
pub fn compare_semver(left: &str, right: &str) -> Ordering {
    match (parse_semver(left), parse_semver(right)) {
        (Some(a), Some(b)) => {
            let core_ord = (a.major, a.minor, a.patch).cmp(&(b.major, b.minor, b.patch));
            if core_ord != Ordering::Equal {
                return core_ord;
            }
            match (&a.prerelease, &b.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(pa), Some(pb)) => compare_prerelease(pa, pb),
            }
        }
        _ => normalize_version_string(left).cmp(&normalize_version_string(right)),
    }
}

/// Returns `true` when `candidate` is strictly newer than `current`.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    let clean_cand = normalize_version_string(candidate);
    if clean_cand.is_empty() {
        return false;
    }
    compare_semver(candidate, current) == Ordering::Greater
}

fn current_platform_keys() -> Vec<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        vec!["darwin-arm64", "macos-arm64", "darwin-universal", "macos-universal"]
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        vec!["darwin-x86_64", "darwin-x64", "macos-x86_64", "macos-x64", "darwin-universal", "macos-universal"]
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        vec!["windows-x86_64", "windows-x64", "win32-x64", "win-x64"]
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        vec!["windows-arm64", "windows-aarch64", "win32-arm64"]
    } else if cfg!(target_os = "linux") {
        vec!["linux-x86_64", "linux-arm64"]
    } else {
        vec!["darwin-arm64", "darwin-universal"]
    }
}

/// Parses a `latest-mac.json` manifest payload into [`AvailableUpdateInfo`] if an asset matches this host.
pub fn parse_release_manifest(json_str: &str) -> Result<AvailableUpdateInfo, String> {
    let manifest: UpdateReleaseManifest =
        serde_json::from_str(json_str).map_err(|e| format!("Invalid manifest JSON: {e}"))?;
    let version = normalize_version_string(&manifest.version);
    if version.is_empty() {
        return Err("Manifest version is empty".to_string());
    }

    let mut chosen: Option<&PlatformAssetEntry> = None;
    for key in current_platform_keys() {
        if let Some(entry) = manifest.platforms.get(key) {
            chosen = Some(entry);
            break;
        }
    }
    if chosen.is_none() {
        chosen = manifest.platforms.values().next();
    }
    let entry = chosen.ok_or_else(|| "Manifest has no platform assets".to_string())?;
    if entry.zip_url.trim().is_empty() {
        return Err("Manifest asset zip_url is empty".to_string());
    }

    Ok(AvailableUpdateInfo {
        version,
        asset_url: entry.zip_url.trim().to_string(),
        sha256: entry
            .zip_sha256
            .as_ref()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty()),
        size_bytes: entry.zip_size,
        release_notes: manifest.release_notes,
        dmg_url: entry.dmg_url.clone(),
        pkg_url: entry.pkg_url.clone(),
    })
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseDto {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GitHubAssetDto>,
}

#[derive(Debug, Deserialize)]
struct GitHubAssetDto {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    digest: Option<String>,
}

fn score_github_asset(name: &str) -> Option<i32> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".blockmap") || lower.ends_with(".sig") || lower.ends_with(".sha256") {
        return None;
    }
    let is_zip = lower.ends_with(".zip");
    let is_tar = lower.ends_with(".tar.gz") || lower.ends_with(".tgz");
    let is_exe = lower.ends_with(".exe");
    if cfg!(target_os = "windows") {
        if !is_zip && !is_exe {
            return None;
        }
        if lower.contains("macos") || lower.contains("darwin") || lower.contains("linux") {
            return None;
        }
        if !lower.contains("windows") && !lower.contains("win64") && !lower.contains("win32") {
            return None;
        }
    } else {
        if !is_zip && !is_tar {
            return None;
        }
        if lower.contains("linux") || lower.contains(".exe") || lower.contains("win32") || lower.contains("windows") {
            return None;
        }
    }
    let arch_score = if cfg!(target_arch = "aarch64") {
        if lower.contains("arm64") || lower.contains("aarch64") {
            40
        } else if lower.contains("universal") {
            30
        } else if lower.contains("x86_64") || lower.contains("x64") {
            return None;
        } else {
            10
        }
    } else if cfg!(target_arch = "x86_64") {
        if lower.contains("x86_64") || lower.contains("x64") {
            40
        } else if lower.contains("universal") {
            30
        } else if lower.contains("arm64") || lower.contains("aarch64") {
            return None;
        } else {
            10
        }
    } else {
        10
    };
    let brand_score = if lower.contains("fabushi") { 20 } else { 5 };
    let fmt_score = if is_zip { 15 } else { 8 };
    Some(arch_score + brand_score + fmt_score)
}

fn extract_sha256_digest(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    let hex = s.strip_prefix("sha256:").unwrap_or(s).trim();
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

/// Parses a GitHub Releases API `/releases/latest` JSON payload into [`AvailableUpdateInfo`].
pub fn parse_github_release_json(json_str: &str) -> Result<AvailableUpdateInfo, String> {
    let dto: GitHubReleaseDto =
        serde_json::from_str(json_str).map_err(|e| format!("Invalid GitHub release JSON: {e}"))?;
    let raw_ver = if !dto.tag_name.trim().is_empty() {
        dto.tag_name.as_str()
    } else {
        dto.name.as_deref().unwrap_or("")
    };
    let version = normalize_version_string(raw_ver);
    if version.is_empty() {
        return Err("GitHub release tag_name is empty".to_string());
    }

    let best = dto
        .assets
        .iter()
        .filter_map(|a| score_github_asset(&a.name).map(|score| (score, a)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, a)| a)
        .ok_or_else(|| "No matching macOS archive asset found in release".to_string())?;

    let dmg_url = dto
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".dmg"))
        .map(|a| a.browser_download_url.clone());
    let pkg_url = dto
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".pkg"))
        .map(|a| a.browser_download_url.clone());

    Ok(AvailableUpdateInfo {
        version,
        asset_url: best.browser_download_url.clone(),
        sha256: extract_sha256_digest(best.digest.as_deref()),
        size_bytes: best.size,
        release_notes: dto.body.filter(|b| !b.trim().is_empty()),
        dmg_url,
        pkg_url,
    })
}

fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let clean = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode_path(clean);
    Some(PathBuf::from(decoded))
}

fn percent_decode_path(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadStageMeta {
    version: String,
    asset_url: String,
    sha256: Option<String>,
    size_bytes: Option<u64>,
}

fn format_reqwest_error(e: &reqwest::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut src = std::error::Error::source(e);
    while let Some(cause) = src {
        let msg = cause.to_string();
        if !msg.is_empty() && !parts.iter().any(|existing| existing.contains(&msg)) {
            parts.push(msg);
        }
        src = cause.source();
    }
    if e.is_timeout() && !parts.iter().any(|p| p.to_ascii_lowercase().contains("timed out")) {
        parts.push("operation timed out".to_string());
    }
    if e.is_connect() && !parts.iter().any(|p| p.to_ascii_lowercase().contains("connect")) {
        parts.push("connection failed".to_string());
    }
    parts.join(": ")
}

fn configure_http_builder(
    mut builder: reqwest::ClientBuilder,
    opts: &UpdaterNetworkOptions,
    use_proxy: bool,
) -> Result<reqwest::ClientBuilder, String> {
    builder = builder.user_agent(format!("Fabushi-Desktop/{CURRENT_APP_VERSION}"));
    if use_proxy {
        if let Some(proxy_cfg) = opts.proxy.as_ref().filter(|p| !p.is_direct()) {
            if let Some(rp) = proxy_cfg
                .to_reqwest_proxy()
                .map_err(|e| format!("Invalid updater proxy configuration: {e}"))?
            {
                builder = builder.proxy(rp);
            }
        }
    }
    Ok(builder)
}

fn build_metadata_http_client(
    opts: &UpdaterNetworkOptions,
    use_proxy: bool,
    total_timeout: Duration,
) -> Result<reqwest::Client, String> {
    let connect_timeout = opts.connect_timeout.unwrap_or(Duration::from_secs(15));
    let builder = configure_http_builder(reqwest::Client::builder(), opts, use_proxy)?
        .connect_timeout(connect_timeout)
        .timeout(total_timeout);
    builder
        .build()
        .map_err(|e| format!("Failed to construct metadata HTTP client: {e}"))
}

fn build_download_http_client(
    opts: &UpdaterNetworkOptions,
    use_proxy: bool,
) -> Result<reqwest::Client, String> {
    let connect_timeout = opts.connect_timeout.unwrap_or(Duration::from_secs(20));
    let idle_timeout = opts.chunk_idle_timeout.unwrap_or(Duration::from_secs(45));
    // Intentionally omit `.timeout(...)` so slow multi-minute downloads succeed while chunks flow.
    let builder = configure_http_builder(reqwest::Client::builder(), opts, use_proxy)?
        .connect_timeout(connect_timeout)
        .read_timeout(idle_timeout)
        .tcp_keepalive(Duration::from_secs(15))
        .http1_only();
    builder
        .build()
        .map_err(|e| format!("Failed to construct download HTTP client: {e}"))
}

fn is_safe_release_segment(seg: &str, max_len: usize) -> bool {
    !seg.is_empty()
        && seg.len() <= max_len
        && !seg.contains("..")
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn extract_github_release_coords<'a>(
    asset_url: &'a str,
    base_override: Option<&str>,
) -> Option<(&'a str, &'a str)> {
    let raw_prefix = base_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("https://github.com/bhrumom/gpui-telegram/releases/download/");
    let trimmed = asset_url.trim();
    let rest = if let Some(r) = trimmed.strip_prefix(raw_prefix) {
        r.trim_start_matches('/')
    } else {
        return None;
    };
    let (tag, filename) = rest.split_once('/')?;
    if filename.contains('/') || !is_safe_release_segment(tag, 64) || !is_safe_release_segment(filename, 128) {
        return None;
    }
    Some((tag, filename))
}

fn valid_http_base_url(server_url: Option<&str>) -> Option<String> {
    let trimmed = server_url.map(str::trim).filter(|s| !s.is_empty())?;
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Some(trimmed.trim_end_matches('/').to_string())
    } else {
        None
    }
}

fn resolve_download_candidates(
    info: &AvailableUpdateInfo,
    opts: &UpdaterNetworkOptions,
) -> Vec<String> {
    let mut candidates = Vec::new();
    if let (Some(base), Some((tag, filename))) = (
        valid_http_base_url(opts.server_url.as_deref()),
        extract_github_release_coords(&info.asset_url, opts.release_download_base_override.as_deref()),
    ) {
        candidates.push(format!("{base}/api/v1/updates/assets/{tag}/{filename}"));
    }
    let primary = info.asset_url.trim().to_string();
    if !candidates.iter().any(|c| c == &primary) {
        candidates.push(primary);
    }
    candidates
}

fn parse_content_range(header_val: &str) -> Option<(u64, u64, Option<u64>)> {
    let trimmed = header_val.trim();
    let rest = trimmed
        .strip_prefix("bytes ")
        .or_else(|| trimmed.strip_prefix("BYTES "))
        .or_else(|| {
            if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("bytes ") {
                Some(&trimmed[6..])
            } else {
                None
            }
        })?
        .trim();
    let (range_part, total_part) = rest.split_once('/')?;
    let (start_s, end_s) = range_part.split_once('-')?;
    let start = start_s.trim().parse::<u64>().ok()?;
    let end = end_s.trim().parse::<u64>().ok()?;
    if start > end {
        return None;
    }
    let total = match total_part.trim() {
        "*" => None,
        other => {
            let t = other.parse::<u64>().ok()?;
            if end >= t {
                return None;
            }
            Some(t)
        }
    };
    Some((start, end, total))
}

fn compute_progress_pct(downloaded: u64, total: Option<u64>) -> u8 {
    match total {
        Some(t) if t > 0 => ((downloaded.saturating_mul(100)) / t).min(100) as u8,
        _ => {
            if downloaded == 0 {
                0
            } else {
                50
            }
        }
    }
}

fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Failed opening archive for SHA-256 verification: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed reading archive for SHA-256 verification: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn prepare_stage_dir(staging_root: &Path, version: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(staging_root).map_err(|e| {
        format!(
            "Failed to create staging root {}: {e}",
            staging_root.display()
        )
    })?;
    let clean_ver = normalize_version_string(version);
    let safe_ver: String = clean_ver
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stage_name = if safe_ver.is_empty() {
        "stage-unknown".to_string()
    } else {
        format!("stage-{safe_ver}")
    };
    if let Ok(entries) = fs::read_dir(staging_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("stage-") && name != stage_name {
                        let _ = fs::remove_dir_all(&path);
                    }
                }
            }
        }
    }
    let stage_dir = staging_root.join(&stage_name);
    fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("Failed to create staging directory: {e}"))?;
    Ok(stage_dir)
}

fn stage_meta_matches(meta_path: &Path, info: &AvailableUpdateInfo) -> bool {
    let Ok(raw) = fs::read_to_string(meta_path) else {
        return false;
    };
    let Ok(meta) = serde_json::from_str::<DownloadStageMeta>(&raw) else {
        return false;
    };
    if normalize_version_string(&meta.version) != normalize_version_string(&info.version) {
        return false;
    }
    match (meta.sha256.as_deref(), info.sha256.as_deref()) {
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        (None, None) => meta.asset_url == info.asset_url,
        _ => false,
    }
}

fn save_stage_meta(meta_path: &Path, info: &AvailableUpdateInfo) {
    let meta = DownloadStageMeta {
        version: info.version.clone(),
        asset_url: info.asset_url.clone(),
        sha256: info.sha256.clone(),
        size_bytes: info.size_bytes,
    };
    if let Ok(json) = serde_json::to_string_pretty(&meta) {
        let _ = fs::write(meta_path, json);
    }
}

fn next_active_candidate(current: usize, total: usize, skipped: &HashSet<usize>) -> usize {
    if total <= 1 {
        return 0;
    }
    for offset in 1..=total {
        let idx = (current + offset) % total;
        if !skipped.contains(&idx) {
            return idx;
        }
    }
    current.min(total - 1)
}

fn retry_backoff_delay(consecutive_stalls: usize, chunk_timeout: Duration) -> Duration {
    let base_ms: u64 = if chunk_timeout < Duration::from_secs(2) {
        25
    } else {
        500
    };
    let shift = consecutive_stalls.saturating_sub(1).min(3);
    Duration::from_millis((base_ms.saturating_mul(1u64 << shift)).min(4000))
}

async fn send_metadata_get(
    url: &str,
    accept_header: Option<&str>,
    opts: &UpdaterNetworkOptions,
    timeout: Duration,
) -> Result<reqwest::Response, String> {
    let has_proxy = opts.proxy.as_ref().is_some_and(|p| !p.is_direct());
    let fallback_direct = opts.proxy.as_ref().is_some_and(|p| p.fallback_to_direct);
    let mut attempt_flags = Vec::with_capacity(2);
    if has_proxy {
        attempt_flags.push(true);
        if fallback_direct {
            attempt_flags.push(false);
        }
    } else {
        attempt_flags.push(false);
    }
    let mut last_err = String::new();
    for use_proxy in attempt_flags {
        let client = build_metadata_http_client(opts, use_proxy, timeout)?;
        let mut req = client.get(url);
        if let Some(accept) = accept_header {
            req = req.header("Accept", accept);
        }
        match req.send().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                last_err = format_reqwest_error(&e);
            }
        }
    }
    Err(last_err)
}

fn parse_any_release_payload(body: &str) -> Result<AvailableUpdateInfo, String> {
    if let Ok(info) = parse_release_manifest(body) {
        return Ok(info);
    }
    parse_github_release_json(body)
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn find_app_bundle_in_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let is_app = p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("app"));
            if is_app && p.join("Contents").join("Info.plist").is_file() {
                return Some(p);
            }
            dirs.push(p);
        }
    }
    for sub in dirs {
        if let Some(found) = find_app_bundle_in_dir(&sub) {
            return Some(found);
        }
    }
    None
}

fn verify_staged_app_bundle(app_dir: &Path) -> Result<(), String> {
    let plist = app_dir.join("Contents").join("Info.plist");
    if !plist.is_file() {
        return Err(format!(
            "Staged bundle missing Info.plist: {}",
            plist.display()
        ));
    }
    let macos_dir = app_dir.join("Contents").join("MacOS");
    let entries = fs::read_dir(&macos_dir)
        .map_err(|e| format!("Staged bundle missing Contents/MacOS: {e}"))?;
    let mut found_exec = false;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
            }
            found_exec = true;
        }
    }
    if !found_exec {
        return Err(format!(
            "Staged bundle contains no executable in {}",
            macos_dir.display()
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("/usr/bin/xattr")
            .args(["-cr", &app_dir.to_string_lossy()])
            .status();
    }
    Ok(())
}

/// Core desktop auto-updater service handling remote discovery, SHA-256 verification, and self-replacement.
pub struct DesktopUpdater;

impl DesktopUpdater {
    /// Checks `feed_override` (if provided) or `DEFAULT_MANIFEST_URL` -> `DEFAULT_GITHUB_LATEST_API_URL`.
    pub async fn check_for_updates(
        feed_override: Option<&str>,
        current_version: &str,
    ) -> Result<Option<AvailableUpdateInfo>, String> {
        Self::check_for_updates_with_options(
            feed_override,
            current_version,
            &UpdaterNetworkOptions::default(),
        )
        .await
    }

    /// Checks for updates using explicit proxy, relay server, and timeout options.
    pub async fn check_for_updates_with_options(
        feed_override: Option<&str>,
        current_version: &str,
        opts: &UpdaterNetworkOptions,
    ) -> Result<Option<AvailableUpdateInfo>, String> {
        let trimmed_override = feed_override.map(str::trim).filter(|s| !s.is_empty());
        let info = if let Some(custom_url) = trimmed_override {
            if let Some(local_path) = file_url_to_path(custom_url) {
                let body = fs::read_to_string(&local_path).map_err(|e| {
                    format!(
                        "Failed to read local update feed {}: {e}",
                        local_path.display()
                    )
                })?;
                parse_any_release_payload(&body)?
            } else {
                let resp = send_metadata_get(custom_url, None, opts, Duration::from_secs(30))
                    .await
                    .map_err(|e| format!("Update check request failed: {e}"))?;
                if !resp.status().is_success() {
                    return Err(format!("Update feed returned HTTP {}", resp.status()));
                }
                let body = resp
                    .text()
                    .await
                    .map_err(|e| format!("Failed reading update feed response: {e}"))?;
                parse_any_release_payload(&body)?
            }
        } else {
            let mut relay_info: Option<AvailableUpdateInfo> = None;
            if let Some(base) = valid_http_base_url(opts.server_url.as_deref()) {
                let relay_url = format!("{base}/api/v1/updates/latest-mac.json");
                if let Ok(resp) =
                    send_metadata_get(&relay_url, None, opts, Duration::from_secs(8)).await
                {
                    if resp.status().is_success() {
                        if let Ok(body) = resp.text().await {
                            if let Ok(parsed) = parse_release_manifest(&body) {
                                relay_info = Some(parsed);
                            }
                        }
                    }
                }
            }

            if let Some(info) = relay_info {
                info
            } else {
                let manifest_attempt = async {
                    let resp = send_metadata_get(
                        DEFAULT_MANIFEST_URL,
                        None,
                        opts,
                        Duration::from_secs(30),
                    )
                    .await
                    .map_err(|e| format!("Manifest check failed: {e}"))?;
                    if !resp.status().is_success() {
                        return Err(format!("Manifest returned HTTP {}", resp.status()));
                    }
                    let body = resp
                        .text()
                        .await
                        .map_err(|e| format!("Failed reading manifest body: {e}"))?;
                    parse_release_manifest(&body)
                }
                .await;

                match manifest_attempt {
                    Ok(info) => info,
                    Err(_) => {
                        let resp = send_metadata_get(
                            DEFAULT_GITHUB_LATEST_API_URL,
                            Some("application/vnd.github+json"),
                            opts,
                            Duration::from_secs(30),
                        )
                        .await
                        .map_err(|e| format!("GitHub release check failed: {e}"))?;
                        if !resp.status().is_success() {
                            return Err(format!(
                                "GitHub releases API returned HTTP {}",
                                resp.status()
                            ));
                        }
                        let body = resp
                            .text()
                            .await
                            .map_err(|e| format!("Failed reading GitHub release response: {e}"))?;
                        parse_github_release_json(&body)?
                    }
                }
            }
        };

        if is_newer_version(current_version, &info.version) {
            Ok(Some(info))
        } else {
            Ok(None)
        }
    }

    /// Streams `info.asset_url`, validates SHA-256 if present, extracts the `.app` bundle, and returns its staged path.
    pub async fn download_and_stage_update(
        info: &AvailableUpdateInfo,
        staging_root: &Path,
        on_progress: impl FnMut(u64, Option<u64>, u8),
    ) -> Result<PathBuf, String> {
        Self::download_and_stage_update_with_options(
            info,
            staging_root,
            &UpdaterNetworkOptions::default(),
            on_progress,
        )
        .await
    }

    /// Resumably streams `info.asset_url` (with HTTP `206` Range resume, proxy/relay failover, and
    /// per-chunk idle timeout recovery), validates SHA-256 if present, extracts the `.app` bundle,
    /// and returns its staged path.
    pub async fn download_and_stage_update_with_options(
        info: &AvailableUpdateInfo,
        staging_root: &Path,
        opts: &UpdaterNetworkOptions,
        mut on_progress: impl FnMut(u64, Option<u64>, u8),
    ) -> Result<PathBuf, String> {
        let stage_dir = prepare_stage_dir(staging_root, &info.version)?;

        let lower_url = info.asset_url.to_ascii_lowercase();
        let archive_ext = if lower_url.ends_with(".tar.gz") || lower_url.ends_with(".tgz") {
            "tar.gz"
        } else {
            "zip"
        };
        let archive_path = stage_dir.join(format!("update.{archive_ext}"));
        let part_path = stage_dir.join(format!("update.{archive_ext}.part"));
        let meta_path = stage_dir.join("download.meta.json");

        if let Some(src_path) = file_url_to_path(&info.asset_url) {
            let _ = fs::remove_file(&part_path);
            let bytes = fs::read(&src_path).map_err(|e| {
                format!(
                    "Failed to read local update archive {}: {e}",
                    src_path.display()
                )
            })?;
            let total = Some(bytes.len() as u64);
            fs::write(&archive_path, &bytes)
                .map_err(|e| format!("Failed to copy local update archive: {e}"))?;
            save_stage_meta(&meta_path, info);
            on_progress(bytes.len() as u64, total, 100);
        } else {
            // Reuse a verified complete archive already on disk if present.
            if archive_path.is_file() {
                let archive_len = fs::metadata(&archive_path).map(|m| m.len()).unwrap_or(0);
                let reuse_ok = if let Some(expected) = info.sha256.as_deref() {
                    compute_file_sha256(&archive_path)
                        .map(|h| h.eq_ignore_ascii_case(expected.trim()))
                        .unwrap_or(false)
                } else {
                    stage_meta_matches(&meta_path, info)
                        && info.size_bytes.is_some_and(|sz| sz > 0 && archive_len == sz)
                };
                if reuse_ok {
                    on_progress(archive_len, Some(archive_len), 100);
                } else {
                    let _ = fs::remove_file(&archive_path);
                }
            }

            if !archive_path.is_file() {
                let max_bytes = info.size_bytes.unwrap_or(u64::MAX);
                let part_len = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
                if !stage_meta_matches(&meta_path, info) || part_len > max_bytes {
                    let _ = fs::remove_file(&part_path);
                }
                save_stage_meta(&meta_path, info);

                // If a previous interrupted run already finished writing all expected bytes, promote it.
                if part_path.is_file() {
                    let cur_len = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
                    if cur_len > 0 && info.size_bytes == Some(cur_len) {
                        if let Some(expected) = info.sha256.as_deref() {
                            if compute_file_sha256(&part_path)
                                .is_ok_and(|h| h.eq_ignore_ascii_case(expected.trim()))
                            {
                                fs::rename(&part_path, &archive_path).map_err(|e| {
                                    format!("Failed finalizing cached partial archive: {e}")
                                })?;
                                on_progress(cur_len, Some(cur_len), 100);
                            } else {
                                let _ = fs::remove_file(&part_path);
                            }
                        }
                    }
                }
            }

            if !archive_path.is_file() {
                let candidates = resolve_download_candidates(info, opts);
                let max_stalls = opts.max_stall_retries.unwrap_or(6).max(1);
                let max_total_segments: usize = 64;
                let chunk_timeout = opts.chunk_idle_timeout.unwrap_or(Duration::from_secs(45));
                let fallback_direct = opts.proxy.as_ref().is_some_and(|p| p.fallback_to_direct);

                let mut skipped_candidates: HashSet<usize> = HashSet::new();
                let mut candidate_idx: usize = 0;
                let mut consecutive_stalls: usize = 0;
                let mut total_segments: usize = 0;
                let mut use_proxy = opts.proxy.as_ref().is_some_and(|p| !p.is_direct());
                let mut last_error: Option<String> = None;
                let mut last_pct: u8 = 255;

                while !archive_path.is_file() {
                    if total_segments >= max_total_segments || consecutive_stalls >= max_stalls {
                        return Err(last_error.unwrap_or_else(|| {
                            "Update download stalled repeatedly across resume attempts".to_string()
                        }));
                    }
                    if skipped_candidates.contains(&candidate_idx) {
                        candidate_idx =
                            next_active_candidate(candidate_idx, candidates.len(), &skipped_candidates);
                    }
                    total_segments += 1;

                    let existing_len = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
                    let client = build_download_http_client(opts, use_proxy)?;
                    let url = &candidates[candidate_idx];
                    let mut req = client
                        .get(url)
                        .header(reqwest::header::ACCEPT_ENCODING, "identity");
                    if existing_len > 0 {
                        req = req.header(reqwest::header::RANGE, format!("bytes={existing_len}-"));
                    }

                    let resp = match req.send().await {
                        Ok(r) => r,
                        Err(e) => {
                            last_error = Some(format!(
                                "Failed downloading update archive: {}",
                                format_reqwest_error(&e)
                            ));
                            if use_proxy && fallback_direct {
                                use_proxy = false;
                            }
                            candidate_idx = next_active_candidate(
                                candidate_idx,
                                candidates.len(),
                                &skipped_candidates,
                            );
                            consecutive_stalls += 1;
                            if consecutive_stalls < max_stalls && total_segments < max_total_segments
                            {
                                tokio::time::sleep(retry_backoff_delay(
                                    consecutive_stalls,
                                    chunk_timeout,
                                ))
                                .await;
                            }
                            continue;
                        }
                    };

                    let status = resp.status();
                    let is_mirror = candidates.len() > 1 && url != &info.asset_url;
                    if is_mirror && matches!(status.as_u16(), 404 | 405 | 501) {
                        skipped_candidates.insert(candidate_idx);
                        candidate_idx = next_active_candidate(
                            candidate_idx,
                            candidates.len(),
                            &skipped_candidates,
                        );
                        continue;
                    }

                    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                        if part_path.is_file() {
                            if let Some(expected) = info.sha256.as_deref() {
                                if compute_file_sha256(&part_path)
                                    .is_ok_and(|h| h.eq_ignore_ascii_case(expected.trim()))
                                {
                                    fs::rename(&part_path, &archive_path).map_err(|e| {
                                        format!("Failed finalizing downloaded archive: {e}")
                                    })?;
                                    let final_len = fs::metadata(&archive_path)
                                        .map(|m| m.len())
                                        .unwrap_or(existing_len);
                                    on_progress(final_len, Some(final_len), 100);
                                    break;
                                }
                            }
                            let _ = fs::remove_file(&part_path);
                        }
                        last_error = Some(format!("Download returned HTTP {status}"));
                        consecutive_stalls += 1;
                        continue;
                    }

                    if !status.is_success() {
                        last_error = Some(format!("Download returned HTTP {status}"));
                        candidate_idx = next_active_candidate(
                            candidate_idx,
                            candidates.len(),
                            &skipped_candidates,
                        );
                        consecutive_stalls += 1;
                        if consecutive_stalls < max_stalls && total_segments < max_total_segments {
                            tokio::time::sleep(retry_backoff_delay(
                                consecutive_stalls,
                                chunk_timeout,
                            ))
                            .await;
                        }
                        continue;
                    }

                    let (mut file, mut downloaded, total) =
                        if status == reqwest::StatusCode::PARTIAL_CONTENT {
                            let parsed_range = resp
                                .headers()
                                .get(reqwest::header::CONTENT_RANGE)
                                .and_then(|v| v.to_str().ok())
                                .and_then(parse_content_range);
                            let Some((start, _end, cr_total)) = parsed_range else {
                                let _ = fs::remove_file(&part_path);
                                last_error = Some(
                                    "Server returned 206 Partial Content without a valid Content-Range header".to_string(),
                                );
                                consecutive_stalls += 1;
                                continue;
                            };
                            if existing_len == 0 || start != existing_len {
                                let _ = fs::remove_file(&part_path);
                                last_error = Some(format!(
                                    "Server Content-Range offset mismatch (requested {existing_len}, got {start})"
                                ));
                                consecutive_stalls += 1;
                                continue;
                            }
                            let file = OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&part_path)
                                .map_err(|e| format!("Failed opening partial archive for append: {e}"))?;
                            let total = cr_total
                                .or_else(|| {
                                    resp.content_length()
                                        .map(|cl| existing_len.saturating_add(cl))
                                })
                                .or(info.size_bytes);
                            (file, existing_len, total)
                        } else {
                            let file = fs::File::create(&part_path)
                                .map_err(|e| format!("Failed creating update archive file: {e}"))?;
                            let total = resp.content_length().or(info.size_bytes);
                            (file, 0, total)
                        };

                    let init_pct = compute_progress_pct(downloaded, total);
                    if init_pct != last_pct {
                        last_pct = init_pct;
                        on_progress(downloaded, total, init_pct);
                    }

                    let mut stream = resp.bytes_stream();
                    let mut bytes_this_segment: u64 = 0;
                    let mut segment_err: Option<String> = None;

                    loop {
                        match tokio::time::timeout(chunk_timeout, stream.next()).await {
                            Ok(Some(Ok(chunk))) => {
                                if !chunk.is_empty() {
                                    file.write_all(&chunk).map_err(|e| {
                                        format!("Disk write error while saving update: {e}")
                                    })?;
                                    downloaded = downloaded.saturating_add(chunk.len() as u64);
                                    bytes_this_segment =
                                        bytes_this_segment.saturating_add(chunk.len() as u64);
                                    let pct = compute_progress_pct(downloaded, total);
                                    if pct != last_pct {
                                        last_pct = pct;
                                        on_progress(downloaded, total, pct);
                                    }
                                }
                            }
                            Ok(Some(Err(e))) => {
                                let _ = file.sync_data();
                                segment_err = Some(format!(
                                    "Stream error while downloading update: {}",
                                    format_reqwest_error(&e)
                                ));
                                break;
                            }
                            Ok(None) => {
                                file.sync_all().map_err(|e| {
                                    format!("Failed flushing downloaded archive: {e}")
                                })?;
                                if let Some(exp_total) = total {
                                    if downloaded < exp_total {
                                        segment_err = Some(format!(
                                            "Update stream ended early ({downloaded}/{exp_total} bytes)"
                                        ));
                                        break;
                                    }
                                }
                                drop(file);
                                fs::rename(&part_path, &archive_path).map_err(|e| {
                                    format!("Failed finalizing downloaded archive: {e}")
                                })?;
                                on_progress(downloaded, total.or(Some(downloaded)), 100);
                                break;
                            }
                            Err(_) => {
                                let _ = file.sync_data();
                                segment_err = Some(format!(
                                    "Stream error while downloading update: stalled for {}s without receiving data",
                                    chunk_timeout.as_secs().max(1)
                                ));
                                break;
                            }
                        }
                    }

                    if archive_path.is_file() {
                        break;
                    }

                    if let Some(err_msg) = segment_err {
                        last_error = Some(err_msg);
                        if bytes_this_segment >= 32 * 1024 {
                            consecutive_stalls = 0;
                        } else {
                            consecutive_stalls += 1;
                            if bytes_this_segment == 0 && use_proxy && fallback_direct {
                                use_proxy = false;
                            }
                            candidate_idx = next_active_candidate(
                                candidate_idx,
                                candidates.len(),
                                &skipped_candidates,
                            );
                            if consecutive_stalls < max_stalls && total_segments < max_total_segments
                            {
                                tokio::time::sleep(retry_backoff_delay(
                                    consecutive_stalls,
                                    chunk_timeout,
                                ))
                                .await;
                            }
                        }
                    }
                }
            }
        }

        if let Some(expected) = info.sha256.as_deref() {
            let actual_hex = compute_file_sha256(&archive_path)?;
            if !actual_hex.eq_ignore_ascii_case(expected.trim()) {
                let _ = fs::remove_file(&archive_path);
                let _ = fs::remove_file(&part_path);
                return Err(format!(
                    "SHA-256 verification failed: expected {}, got {}",
                    expected, actual_hex
                ));
            }
        }

        let extract_dir = stage_dir.join("extracted");
        if extract_dir.exists() {
            let _ = fs::remove_dir_all(&extract_dir);
        }
        fs::create_dir_all(&extract_dir)
            .map_err(|e| format!("Failed creating extraction dir: {e}"))?;

        let status = if archive_ext == "zip" {
            if Path::new("/usr/bin/ditto").exists() {
                Command::new("/usr/bin/ditto")
                    .args([
                        "-x",
                        "-k",
                        &archive_path.to_string_lossy(),
                        &extract_dir.to_string_lossy(),
                    ])
                    .status()
            } else {
                Command::new("unzip")
                    .args([
                        "-q",
                        "-o",
                        &archive_path.to_string_lossy(),
                        "-d",
                        &extract_dir.to_string_lossy(),
                    ])
                    .status()
            }
        } else {
            Command::new("tar")
                .args([
                    "-xzf",
                    &archive_path.to_string_lossy(),
                    "-C",
                    &extract_dir.to_string_lossy(),
                ])
                .status()
        }
        .map_err(|e| format!("Failed launching archive extractor: {e}"))?;

        if !status.success() {
            return Err(format!("Archive extraction exited with status {status}"));
        }

        let staged_app = find_app_bundle_in_dir(&extract_dir)
            .ok_or_else(|| "Extracted update archive did not contain a valid .app bundle".to_string())?;
        verify_staged_app_bundle(&staged_app)?;
        let _ = fs::remove_file(&archive_path);
        Ok(staged_app)
    }

    /// Resolves which `.app` bundle path should be replaced on update given the running executable path.
    pub fn resolve_target_app_bundle(exe_path: &Path) -> PathBuf {
        for anc in exe_path.ancestors() {
            let is_app = anc
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("app"));
            if is_app && anc.join("Contents").join("MacOS").is_dir() {
                return anc.to_path_buf();
            }
        }
        let sys_apps = PathBuf::from("/Applications/Fabushi.app");
        let sys_writable = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open("/Applications/.fabushi_write_probe");
        match sys_writable {
            Ok(_) => {
                let _ = fs::remove_file("/Applications/.fabushi_write_probe");
                sys_apps
            }
            Err(_) => {
                if let Ok(home) = std::env::var("HOME") {
                    let user_apps = PathBuf::from(home).join("Applications");
                    let _ = fs::create_dir_all(&user_apps);
                    user_apps.join("Fabushi.app")
                } else {
                    sys_apps
                }
            }
        }
    }

    /// Atomically replaces `target_app` with `staged_app` synchronously (with rollback if replacement fails).
    pub fn replace_app_bundle_in_place(staged_app: &Path, target_app: &Path) -> Result<(), String> {
        verify_staged_app_bundle(staged_app)?;
        if let Some(parent) = target_app.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed creating target parent directory: {e}"))?;
        }
        let backup_app = target_app.with_extension("app.bak");
        if backup_app.exists() {
            let _ = fs::remove_dir_all(&backup_app);
        }
        let had_existing = target_app.exists();
        if had_existing {
            fs::rename(target_app, &backup_app)
                .map_err(|e| format!("Failed moving existing bundle to backup: {e}"))?;
        }
        if let Err(e) = fs::rename(staged_app, target_app) {
            if had_existing && backup_app.exists() {
                let _ = fs::rename(&backup_app, target_app);
            }
            return Err(format!("Failed moving staged bundle into place: {e}"));
        }
        if backup_app.exists() {
            let _ = fs::remove_dir_all(&backup_app);
        }
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("/usr/bin/xattr")
                .args(["-cr", &target_app.to_string_lossy()])
                .status();
            let temp_root = fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
            let canon_target = fs::canonicalize(target_app).unwrap_or_else(|_| target_app.to_path_buf());
            let in_temp_dir = canon_target.starts_with(&temp_root) || target_app.starts_with(std::env::temp_dir());
            if !in_temp_dir && std::env::var_os("FABUSHI_SKIP_OS_REGISTER").is_none() {
                let lsreg = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister";
                if Path::new(lsreg).exists() {
                    let _ = Command::new(lsreg)
                        .args(["-f", &target_app.to_string_lossy()])
                        .status();
                }
                if Path::new("/usr/bin/mdimport").exists() {
                    let _ = Command::new("/usr/bin/mdimport")
                        .arg(&target_app.to_string_lossy().into_owned())
                        .status();
                }
            }
        }
        Ok(())
    }

    /// Spawns a detached POSIX helper that waits for `current_pid` to exit, atomically swaps
    /// `staged_app` into `target_app`, re-registers LaunchServices/Spotlight, and optionally relaunches.
    pub fn apply_staged_update_and_relaunch(
        staged_app: &Path,
        target_app: &Path,
        current_pid: u32,
        relaunch: bool,
    ) -> Result<PathBuf, String> {
        verify_staged_app_bundle(staged_app)?;
        let stage_parent = staged_app
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or_else(|| Path::new("/tmp"));
        let script_path = stage_parent.join("apply_update.sh");

        let staged_q = shell_single_quote(&staged_app.to_string_lossy());
        let target_q = shell_single_quote(&target_app.to_string_lossy());
        let backup_q = shell_single_quote(&target_app.with_extension("app.bak").to_string_lossy());
        let stage_dir_q = shell_single_quote(&stage_parent.to_string_lossy());
        let target_parent_q = shell_single_quote(
            &target_app
                .parent()
                .unwrap_or_else(|| Path::new("/Applications"))
                .to_string_lossy(),
        );
        let relaunch_line = if relaunch {
            format!("/usr/bin/open -n {target_q} >/dev/null 2>&1 || true")
        } else {
            String::new()
        };

        let script = format!(
            r#"#!/bin/sh
set -u
OLD_PID={current_pid}
STAGED={staged_q}
TARGET={target_q}
BACKUP={backup_q}
TARGET_PARENT={target_parent_q}
STAGE_DIR={stage_dir_q}

i=0
while [ "$i" -lt 60 ]; do
  if ! kill -0 "$OLD_PID" 2>/dev/null; then
    break
  fi
  sleep 0.25
  i=$((i + 1))
done

mkdir -p "$TARGET_PARENT" || true
rm -rf "$BACKUP" || true

swap_bundles() {{
  if [ -e "$TARGET" ]; then
    mv "$TARGET" "$BACKUP" || return 1
  fi
  if ! mv "$STAGED" "$TARGET"; then
    if [ -e "$BACKUP" ]; then
      mv "$BACKUP" "$TARGET" || true
    fi
    return 1
  fi
  rm -rf "$BACKUP" || true
  return 0
}}

if ! swap_bundles; then
  if command -v osascript >/dev/null 2>&1; then
    CMD="mkdir -p '$TARGET_PARENT' && rm -rf '$BACKUP' && ([ ! -e '$TARGET' ] || mv '$TARGET' '$BACKUP') && mv '$STAGED' '$TARGET' && rm -rf '$BACKUP'"
    ESCAPED=$(printf '%s' "$CMD" | sed 's/\\/\\\\/g; s/"/\\"/g')
    if ! osascript -e "do shell script \"$ESCAPED\" with administrator privileges" >/dev/null 2>&1; then
      if [ -e "$BACKUP" ] && [ ! -e "$TARGET" ]; then
        mv "$BACKUP" "$TARGET" || true
      fi
      exit 1
    fi
  else
    exit 1
  fi
fi

/usr/bin/xattr -cr "$TARGET" >/dev/null 2>&1 || true
LSREG="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
if [ -x "$LSREG" ]; then
  "$LSREG" -f "$TARGET" >/dev/null 2>&1 || true
fi
if command -v mdimport >/dev/null 2>&1; then
  mdimport "$TARGET" >/dev/null 2>&1 || true
fi
{relaunch_line}
rm -rf "$STAGE_DIR" >/dev/null 2>&1 || true
"#
        );

        fs::write(&script_path, script.as_bytes())
            .map_err(|e| format!("Failed writing updater helper script: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("Failed marking updater script executable: {e}"))?;
        }

        Command::new("/bin/sh")
            .arg(&script_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed spawning updater helper process: {e}"))?;

        Ok(script_path)
    }
}
