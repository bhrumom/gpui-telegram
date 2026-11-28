//! Dynamic OS system-proxy & environment-proxy aware HTTP client builder.
//!
//! Why this exists alongside `reqwest`'s `"system-proxy"` feature:
//! 1. macOS GUI `.app` bundles launched from Finder/Dock do NOT inherit shell `HTTPS_PROXY` /
//!    `HTTP_PROXY` environment variables.
//! 2. `hyper-util`'s built-in `client-proxy-system` snapshots system proxy once at `Client::build()`
//!    time (missing runtime Clash/Surge/VPN toggles), does NOT read macOS `SOCKSEnable` /
//!    `SOCKSProxy`, and does NOT populate macOS `ExceptionsList` (`NO_PROXY`), which would
//!    otherwise route `127.0.0.1` / `localhost` through the user's HTTP proxy.
//! 3. This module resolves env vars + optional in-app proxy + live OS system proxy (macOS
//!    `scutil --proxy` HTTP/HTTPS/SOCKS5h + `ExceptionsList`, Windows Registry `Internet Settings`
//!    including per-protocol `http=...;https=...;socks=...` + `ProxyOverride`) with a short TTL
//!    cache (invalidatable on demand) and unconditional loopback safety.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Short cache TTL for OS system proxy queries so toggling Clash/Surge/VPN takes effect
/// immediately without restarting the desktop app, while avoiding redundant OS queries during
/// concurrent requests.
const SYSTEM_PROXY_CACHE_TTL: Duration = Duration::from_secs(3);

/// Resolved proxy configuration (from env vars, in-app override, or OS system settings).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemProxyConfig {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub no_proxy: Vec<String>,
    pub exclude_simple_hostnames: bool,
}

impl SystemProxyConfig {
    pub fn is_empty(&self) -> bool {
        self.http_proxy.is_none() && self.https_proxy.is_none() && self.all_proxy.is_none()
    }

    /// Creates a config routing both HTTP and HTTPS traffic through the given single proxy URL
    /// (e.g., `http://127.0.0.1:7890` or `socks5h://127.0.0.1:1080`).
    pub fn from_all_proxy_url(url: impl Into<String>) -> Self {
        let raw = url.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        let normalized = normalize_proxy_value(trimmed, "http").unwrap_or_else(|| trimmed.to_string());
        Self {
            http_proxy: Some(normalized.clone()),
            https_proxy: Some(normalized.clone()),
            all_proxy: Some(normalized),
            no_proxy: Vec::new(),
            exclude_simple_hostnames: false,
        }
    }
}

static OS_PROXY_CACHE: LazyLock<Mutex<Option<(Instant, SystemProxyConfig)>>> =
    LazyLock::new(|| Mutex::new(None));

static APP_PROXY_OVERRIDE: LazyLock<Mutex<Option<SystemProxyConfig>>> =
    LazyLock::new(|| Mutex::new(None));

/// Clears the cached OS system proxy snapshot so the very next HTTP request reads fresh OS
/// network/proxy settings immediately (e.g., when clicking "Refresh & Activate" after enabling
/// a VPN or Clash/Surge proxy).
pub fn invalidate_system_proxy_cache() {
    if let Ok(mut guard) = OS_PROXY_CACHE.lock() {
        *guard = None;
    }
}

/// Installs or clears an in-app proxy override (e.g. configured in Fabushi Connection Settings).
/// Environment variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`) still take precedence.
pub fn set_app_proxy_override(proxy: Option<SystemProxyConfig>) {
    if let Ok(mut guard) = APP_PROXY_OVERRIDE.lock() {
        *guard = proxy;
    }
}

/// Normalizes a proxy host/URL string by ensuring it has an explicit scheme and brackets IPv6
/// literals when combining host + port.
pub fn format_proxy_endpoint(scheme: &str, host: &str, port: Option<u16>) -> Option<String> {
    let raw = host.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains("://") {
        return Some(raw.to_string());
    }
    let formatted_host = if raw.contains(':') && !raw.starts_with('[') && !raw.ends_with(']') {
        // Distinguish bare IPv6 literal (multiple colons) from `host:port` (single colon).
        if raw.matches(':').count() > 1 {
            format!("[{raw}]")
        } else {
            return Some(format!("{scheme}://{raw}"));
        }
    } else {
        raw.to_string()
    };
    match port.filter(|p| *p > 0) {
        Some(p) => Some(format!("{scheme}://{formatted_host}:{p}")),
        None => Some(format!("{scheme}://{formatted_host}")),
    }
}

fn normalize_proxy_value(raw: &str, default_scheme: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        return Some(trimmed.to_string());
    }
    format_proxy_endpoint(default_scheme, trimmed, None)
}

/// Parses the output of macOS `/usr/sbin/scutil --proxy` into a [`SystemProxyConfig`].
///
/// Supports:
/// - `HTTPEnable` / `HTTPProxy` / `HTTPPort` -> `http://host:port`
/// - `HTTPSEnable` / `HTTPSProxy` / `HTTPSPort` -> `http://host:port` (HTTP CONNECT tunnel)
/// - `SOCKSEnable` / `SOCKSProxy` / `SOCKSPort` -> `socks5h://host:port` (proxy-side DNS)
/// - `ExcludeSimpleHostnames`
/// - `ExceptionsList` array entries
pub fn parse_macos_scutil_proxy_output(output: &str) -> SystemProxyConfig {
    let mut http_enable = false;
    let mut http_host: Option<String> = None;
    let mut http_port: Option<u16> = None;

    let mut https_enable = false;
    let mut https_host: Option<String> = None;
    let mut https_port: Option<u16> = None;

    let mut socks_enable = false;
    let mut socks_host: Option<String> = None;
    let mut socks_port: Option<u16> = None;

    let mut exclude_simple_hostnames = false;
    let mut no_proxy = Vec::new();
    let mut in_exceptions_list = false;

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if in_exceptions_list {
            if line == "}" {
                in_exceptions_list = false;
                continue;
            }
            if let Some((_, val)) = line.split_once(':') {
                let entry = val.trim().trim_matches('"');
                if !entry.is_empty() {
                    no_proxy.push(entry.to_string());
                }
            }
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        match key {
            "ExceptionsList" => {
                if val.contains('{') && !val.contains('}') {
                    in_exceptions_list = true;
                }
            }
            "ExcludeSimpleHostnames" => {
                exclude_simple_hostnames = val == "1" || val.eq_ignore_ascii_case("true");
            }
            "HTTPEnable" => {
                http_enable = val == "1" || val.eq_ignore_ascii_case("true");
            }
            "HTTPProxy" => {
                if !val.is_empty() {
                    http_host = Some(val.trim_matches('"').to_string());
                }
            }
            "HTTPPort" => {
                http_port = val.parse::<u16>().ok();
            }
            "HTTPSEnable" => {
                https_enable = val == "1" || val.eq_ignore_ascii_case("true");
            }
            "HTTPSProxy" => {
                if !val.is_empty() {
                    https_host = Some(val.trim_matches('"').to_string());
                }
            }
            "HTTPSPort" => {
                https_port = val.parse::<u16>().ok();
            }
            "SOCKSEnable" => {
                socks_enable = val == "1" || val.eq_ignore_ascii_case("true");
            }
            "SOCKSProxy" => {
                if !val.is_empty() {
                    socks_host = Some(val.trim_matches('"').to_string());
                }
            }
            "SOCKSPort" => {
                socks_port = val.parse::<u16>().ok();
            }
            _ => {}
        }
    }

    let http_proxy = if http_enable {
        http_host
            .as_deref()
            .and_then(|h| format_proxy_endpoint("http", h, http_port))
    } else {
        None
    };
    // macOS Secure Web Proxy (HTTPSProxy) speaks HTTP CONNECT on the proxy hop, so its URI scheme
    // is `http://` unless the user explicitly specified `https://`.
    let https_proxy = if https_enable {
        https_host
            .as_deref()
            .and_then(|h| format_proxy_endpoint("http", h, https_port))
    } else {
        None
    };
    let all_proxy = if socks_enable {
        socks_host
            .as_deref()
            .and_then(|h| format_proxy_endpoint("socks5h", h, socks_port))
    } else {
        None
    };

    SystemProxyConfig {
        http_proxy,
        https_proxy,
        all_proxy,
        no_proxy,
        exclude_simple_hostnames,
    }
}

/// Parses Windows Internet Settings (`ProxyEnable`, `ProxyServer`, `ProxyOverride`) into a
/// [`SystemProxyConfig`], supporting both unified (`127.0.0.1:7890`) and per-protocol
/// (`http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7891`) registry values.
pub fn parse_windows_proxy_settings(
    proxy_enable: u32,
    proxy_server: &str,
    proxy_override: Option<&str>,
) -> SystemProxyConfig {
    if proxy_enable == 0 {
        return SystemProxyConfig::default();
    }
    let mut cfg = SystemProxyConfig::default();
    let trimmed = proxy_server.trim();
    if !trimmed.is_empty() {
        if trimmed.contains('=') {
            for part in trimmed.split(';') {
                let Some((proto, addr)) = part.trim().split_once('=') else {
                    continue;
                };
                match proto.trim().to_ascii_lowercase().as_str() {
                    "http" => {
                        cfg.http_proxy = normalize_proxy_value(addr, "http");
                    }
                    "https" => {
                        cfg.https_proxy = normalize_proxy_value(addr, "http");
                    }
                    "socks" | "socks5" | "socks5h" => {
                        cfg.all_proxy = normalize_proxy_value(addr, "socks5h");
                    }
                    "socks4" | "socks4a" => {
                        cfg.all_proxy = normalize_proxy_value(addr, "socks4a");
                    }
                    _ => {}
                }
            }
        } else if trimmed.to_ascii_lowercase().starts_with("socks") {
            cfg.all_proxy = normalize_proxy_value(trimmed, "socks5h");
        } else {
            let normalized = normalize_proxy_value(trimmed, "http");
            cfg.http_proxy = normalized.clone();
            cfg.https_proxy = normalized;
        }
    }

    if let Some(raw_override) = proxy_override {
        for part in raw_override.split([';', ',']) {
            let item = part.trim();
            if item.is_empty() {
                continue;
            }
            if item.eq_ignore_ascii_case("<local>") {
                cfg.exclude_simple_hostnames = true;
            } else {
                cfg.no_proxy.push(item.to_string());
            }
        }
    }

    cfg
}

/// Parses `reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings"` output.
pub fn parse_windows_reg_query_output(output: &str) -> SystemProxyConfig {
    let mut enable: u32 = 0;
    let mut server = String::new();
    let mut override_list: Option<String> = None;

    for raw_line in output.lines() {
        let line = raw_line.trim();
        let mut parts = line.splitn(3, char::is_whitespace).filter(|s| !s.is_empty());
        let Some(name) = parts.next() else {
            continue;
        };
        let rest = line[name.len()..].trim_start();
        let Some((reg_type, val_part)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        if !reg_type.starts_with("REG_") {
            continue;
        }
        let val = val_part.trim();
        if name.eq_ignore_ascii_case("ProxyEnable") {
            enable = if let Some(hex) = val.strip_prefix("0x").or_else(|| val.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).unwrap_or(0)
            } else {
                val.parse::<u32>().unwrap_or(0)
            };
        } else if name.eq_ignore_ascii_case("ProxyServer") {
            server = val.to_string();
        } else if name.eq_ignore_ascii_case("ProxyOverride") && !val.is_empty() {
            override_list = Some(val.to_string());
        }
    }

    parse_windows_proxy_settings(enable, &server, override_list.as_deref())
}

fn read_first_env(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(val) = std::env::var(name) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Reads standard curl-compatible proxy environment variables (`HTTPS_PROXY`, `HTTP_PROXY`,
/// `ALL_PROXY`, `NO_PROXY`, upper or lower case).
pub fn parse_proxy_config_from_env() -> SystemProxyConfig {
    let http_proxy =
        read_first_env(&["HTTP_PROXY", "http_proxy"]).and_then(|v| normalize_proxy_value(&v, "http"));
    let https_proxy = read_first_env(&["HTTPS_PROXY", "https_proxy"])
        .and_then(|v| normalize_proxy_value(&v, "http"));
    let all_proxy =
        read_first_env(&["ALL_PROXY", "all_proxy"]).and_then(|v| normalize_proxy_value(&v, "http"));
    let no_proxy = read_first_env(&["NO_PROXY", "no_proxy"])
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    SystemProxyConfig {
        http_proxy,
        https_proxy,
        all_proxy,
        no_proxy,
        exclude_simple_hostnames: false,
    }
}

#[cfg(target_os = "macos")]
fn query_os_system_proxy_uncached() -> SystemProxyConfig {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .arg("--proxy")
        .output()
        .or_else(|_| std::process::Command::new("scutil").arg("--proxy").output());
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_macos_scutil_proxy_output(&text)
        }
        _ => SystemProxyConfig::default(),
    }
}

#[cfg(target_os = "windows")]
fn query_os_system_proxy_uncached() -> SystemProxyConfig {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_windows_reg_query_output(&text)
        }
        _ => SystemProxyConfig::default(),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn query_os_system_proxy_uncached() -> SystemProxyConfig {
    SystemProxyConfig::default()
}

fn read_os_system_proxy_cached() -> SystemProxyConfig {
    if std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
        return SystemProxyConfig::default();
    }
    if let Ok(guard) = OS_PROXY_CACHE.lock() {
        if let Some((fetched_at, ref cfg)) = *guard {
            if fetched_at.elapsed() < SYSTEM_PROXY_CACHE_TTL {
                return cfg.clone();
            }
        }
    }
    let fresh = query_os_system_proxy_uncached();
    if let Ok(mut guard) = OS_PROXY_CACHE.lock() {
        *guard = Some((Instant::now(), fresh.clone()));
    }
    fresh
}

fn merge_proxy_configs(base: SystemProxyConfig, overlay: SystemProxyConfig) -> SystemProxyConfig {
    let mut no_proxy = base.no_proxy;
    for entry in overlay.no_proxy {
        if !no_proxy.iter().any(|e| e.eq_ignore_ascii_case(&entry)) {
            no_proxy.push(entry);
        }
    }
    SystemProxyConfig {
        http_proxy: overlay.http_proxy.or(base.http_proxy),
        https_proxy: overlay.https_proxy.or(base.https_proxy),
        all_proxy: overlay.all_proxy.or(base.all_proxy),
        no_proxy,
        exclude_simple_hostnames: base.exclude_simple_hostnames || overlay.exclude_simple_hostnames,
    }
}

/// Returns the currently active merged proxy configuration:
/// `OS system proxy` < `in-app proxy override` < `process env vars`.
pub fn current_effective_proxy_config() -> SystemProxyConfig {
    let mut cfg = read_os_system_proxy_cached();
    if let Ok(guard) = APP_PROXY_OVERRIDE.lock() {
        if let Some(ref app_override) = *guard {
            cfg = merge_proxy_configs(cfg, app_override.clone());
        }
    }
    let env_cfg = parse_proxy_config_from_env();
    merge_proxy_configs(cfg, env_cfg)
}

fn is_loopback_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") || h == "0.0.0.0" {
        return true;
    }
    h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn parse_ipv4_cidr_rule(rule: &str) -> Option<(u32, u32)> {
    // Supports standard CIDR (`10.0.0.0/8`, `192.168.0.0/16`), macOS shorthand CIDR
    // (`169.254/16`, `10/8`, `192.168.1/24`), and Windows IPv4 wildcards (`127.*`, `192.168.*`).
    if let Some((ip_part, prefix_str)) = rule.split_once('/') {
        let prefix: u32 = prefix_str.parse().ok()?;
        if prefix > 32 {
            return None;
        }
        let octets_str: Vec<&str> = ip_part.split('.').collect();
        if octets_str.is_empty() || octets_str.len() > 4 {
            return None;
        }
        let mut octets = [0u8; 4];
        for (idx, seg) in octets_str.iter().enumerate() {
            octets[idx] = seg.parse::<u8>().ok()?;
        }
        let net = u32::from(Ipv4Addr::from(octets));
        let mask = if prefix == 0 {
            0u32
        } else {
            !0u32 << (32 - prefix)
        };
        return Some((net & mask, mask));
    }

    if rule.contains('*') {
        let parts: Vec<&str> = rule.split('.').collect();
        let first_wild = parts.iter().position(|p| *p == "*")?;
        if first_wild == 0 || first_wild > 3 || parts[first_wild..].iter().any(|p| *p != "*") {
            return None;
        }
        let mut octets = [0u8; 4];
        for (idx, seg) in parts[..first_wild].iter().enumerate() {
            octets[idx] = seg.parse::<u8>().ok()?;
        }
        let prefix = (first_wild as u32) * 8;
        let net = u32::from(Ipv4Addr::from(octets));
        let mask = !0u32 << (32 - prefix);
        return Some((net & mask, mask));
    }

    None
}

fn parse_ipv6_cidr_rule(rule: &str) -> Option<(u128, u128)> {
    let (ip_part, prefix_str) = rule.split_once('/')?;
    let prefix: u32 = prefix_str.parse().ok()?;
    if prefix > 128 {
        return None;
    }
    let ip: Ipv6Addr = ip_part
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()?;
    let net = u128::from(ip);
    let mask = if prefix == 0 {
        0u128
    } else {
        !0u128 << (128 - prefix)
    };
    Some((net & mask, mask))
}

/// Checks whether `host` should bypass proxying given `no_proxy` rules, `exclude_simple_hostnames`,
/// and `bypass_loopback`.
pub fn host_matches_no_proxy(
    host: &str,
    no_proxy: &[String],
    exclude_simple_hostnames: bool,
    bypass_loopback: bool,
) -> bool {
    let clean_host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if clean_host.is_empty() {
        return true;
    }
    if bypass_loopback && is_loopback_host(clean_host) {
        return true;
    }
    let host_ip = clean_host.parse::<IpAddr>().ok();
    if exclude_simple_hostnames && host_ip.is_none() && !clean_host.contains('.') {
        return true;
    }

    for raw_rule in no_proxy {
        let rule = raw_rule.trim();
        if rule.is_empty() {
            continue;
        }
        if rule == "*" {
            return true;
        }
        if let Some(ip) = host_ip {
            match ip {
                IpAddr::V4(v4) => {
                    if let Some((net, mask)) = parse_ipv4_cidr_rule(rule) {
                        if (u32::from(v4) & mask) == net {
                            return true;
                        }
                        continue;
                    }
                    if let Ok(IpAddr::V4(rule_v4)) = rule.parse::<IpAddr>() {
                        if v4 == rule_v4 {
                            return true;
                        }
                    }
                }
                IpAddr::V6(v6) => {
                    if let Some((net, mask)) = parse_ipv6_cidr_rule(rule) {
                        if (u128::from(v6) & mask) == net {
                            return true;
                        }
                        continue;
                    }
                    let bare = rule.trim_start_matches('[').trim_end_matches(']');
                    if let Ok(IpAddr::V6(rule_v6)) = bare.parse::<IpAddr>() {
                        if v6 == rule_v6 {
                            return true;
                        }
                    }
                }
            }
            continue;
        }

        // Domain suffix rule (supports `*.example.com`, `.example.com`, `example.com`)
        let suffix = rule
            .strip_prefix("*.")
            .or_else(|| rule.strip_prefix('.'))
            .unwrap_or(rule);
        if suffix.is_empty() {
            continue;
        }
        if clean_host.eq_ignore_ascii_case(suffix) {
            return true;
        }
        if clean_host.len() > suffix.len() {
            let split_idx = clean_host.len() - suffix.len();
            if clean_host.as_bytes().get(split_idx - 1) == Some(&b'.')
                && clean_host[split_idx..].eq_ignore_ascii_case(suffix)
            {
                return true;
            }
        }
    }

    false
}

/// Resolves the proxy URL (if any) that should be used for `target` under `cfg`.
pub fn resolve_proxy_url_for_target(
    cfg: &SystemProxyConfig,
    target: &reqwest::Url,
    bypass_loopback: bool,
) -> Option<String> {
    if cfg.is_empty() {
        return None;
    }
    let host = target.host_str()?;
    if host_matches_no_proxy(
        host,
        &cfg.no_proxy,
        cfg.exclude_simple_hostnames,
        bypass_loopback,
    ) {
        return None;
    }
    match target.scheme() {
        "https" => cfg.https_proxy.clone().or_else(|| cfg.all_proxy.clone()),
        "http" => cfg.http_proxy.clone().or_else(|| cfg.all_proxy.clone()),
        _ => None,
    }
}

fn resolve_dynamic_proxy_for_url(target: &reqwest::Url) -> Option<String> {
    let cfg = current_effective_proxy_config();
    resolve_proxy_url_for_target(&cfg, target, true)
}

/// Returns a [`reqwest::ClientBuilder`] pre-configured with our dynamic, loopback-safe,
/// cross-platform (macOS HTTP/HTTPS/SOCKS5h + `ExceptionsList`, Windows Registry, Linux env vars)
/// proxy resolver.
pub fn build_proxy_aware_http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::custom(resolve_dynamic_proxy_for_url))
}

/// Builds a proxy-aware [`reqwest::Client`] with optional connect and overall request timeouts.
pub fn build_proxy_aware_http_client(
    connect_timeout: Option<Duration>,
    timeout: Option<Duration>,
) -> reqwest::Client {
    let mut builder = build_proxy_aware_http_client_builder();
    if let Some(ct) = connect_timeout {
        builder = builder.connect_timeout(ct);
    }
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }
    builder
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_macos_scutil_clash_http_https_socks_and_exceptions() {
        let sample = r#"
<dictionary> {
  ExceptionsList : <array> {
    0 : 127.0.0.1
    1 : localhost
    2 : *.local
    3 : 169.254/16
    4 : 192.168.0.0/16
  }
  ExcludeSimpleHostnames : 1
  FTPPassive : 1
  HTTPEnable : 1
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7890
  HTTPSProxy : 127.0.0.1
  SOCKSEnable : 1
  SOCKSPort : 7891
  SOCKSProxy : 127.0.0.1
}
"#;
        let cfg = parse_macos_scutil_proxy_output(sample);
        assert_eq!(cfg.http_proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.https_proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.all_proxy.as_deref(), Some("socks5h://127.0.0.1:7891"));
        assert!(cfg.exclude_simple_hostnames);
        assert_eq!(
            cfg.no_proxy,
            vec!["127.0.0.1", "localhost", "*.local", "169.254/16", "192.168.0.0/16"]
        );

        // Google & OpenAI OAuth endpoints must route through proxy
        let google_url = reqwest::Url::parse("https://oauth2.googleapis.com/token").unwrap();
        assert_eq!(
            resolve_proxy_url_for_target(&cfg, &google_url, true).as_deref(),
            Some("http://127.0.0.1:7890")
        );
        let openai_url = reqwest::Url::parse("https://auth.openai.com/oauth/token").unwrap();
        assert_eq!(
            resolve_proxy_url_for_target(&cfg, &openai_url, true).as_deref(),
            Some("http://127.0.0.1:7890")
        );

        // Loopback, LAN, link-local, *.local, and simple hostnames must bypass proxy
        for direct in [
            "http://127.0.0.1:8080/api/v1/hermes/magpie/state",
            "http://localhost:11434/v1/models",
            "http://[::1]:1455/auth/callback",
            "http://192.168.1.42:8080/health",
            "http://169.254.169.254/latest/meta-data",
            "http://macbook.local:8080/ping",
            "http://intranet:8080/v1",
        ] {
            let u = reqwest::Url::parse(direct).unwrap();
            assert_eq!(
                resolve_proxy_url_for_target(&cfg, &u, true),
                None,
                "expected direct bypass for {direct}"
            );
        }
    }

    #[test]
    fn parse_macos_scutil_socks_only_routes_https_and_http_via_socks5h() {
        let sample = r#"
<dictionary> {
  HTTPEnable : 0
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 0
  HTTPSPort : 7890
  HTTPSProxy : 127.0.0.1
  SOCKSEnable : 1
  SOCKSPort : 1080
  SOCKSProxy : 127.0.0.1
}
"#;
        let cfg = parse_macos_scutil_proxy_output(sample);
        assert_eq!(cfg.http_proxy, None);
        assert_eq!(cfg.https_proxy, None);
        assert_eq!(cfg.all_proxy.as_deref(), Some("socks5h://127.0.0.1:1080"));

        let google = reqwest::Url::parse("https://oauth2.googleapis.com/token").unwrap();
        assert_eq!(
            resolve_proxy_url_for_target(&cfg, &google, true).as_deref(),
            Some("socks5h://127.0.0.1:1080")
        );
        let loopback = reqwest::Url::parse("http://127.0.0.1:43210/api/v1/health").unwrap();
        assert_eq!(resolve_proxy_url_for_target(&cfg, &loopback, true), None);
    }

    #[test]
    fn parse_windows_reg_query_per_protocol_and_override() {
        let sample = r#"
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Internet Settings
    ProxyEnable    REG_DWORD    0x1
    ProxyServer    REG_SZ    http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7891
    ProxyOverride  REG_SZ    localhost;127.*;10.*;192.168.*;*.corp.internal;<local>
"#;
        let cfg = parse_windows_reg_query_output(sample);
        assert_eq!(cfg.http_proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.https_proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.all_proxy.as_deref(), Some("socks5h://127.0.0.1:7891"));
        assert!(cfg.exclude_simple_hostnames);

        let ext = reqwest::Url::parse("https://chatgpt.com/backend-api/codex/responses").unwrap();
        assert_eq!(
            resolve_proxy_url_for_target(&cfg, &ext, true).as_deref(),
            Some("http://127.0.0.1:7890")
        );
        for direct in [
            "http://10.12.34.56:8080/v1",
            "http://192.168.50.2/v1",
            "https://api.corp.internal/v1",
            "http://devbox/v1",
        ] {
            let u = reqwest::Url::parse(direct).unwrap();
            assert_eq!(
                resolve_proxy_url_for_target(&cfg, &u, true),
                None,
                "expected direct bypass for {direct}"
            );
        }

        // Disabled Windows proxy yields empty config
        let disabled = parse_windows_proxy_settings(0, "127.0.0.1:7890", Some("<local>"));
        assert!(disabled.is_empty());
    }

    #[test]
    fn build_proxy_aware_http_client_constructs_cleanly() {
        let _client = build_proxy_aware_http_client(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(15)),
        );
        invalidate_system_proxy_cache();
    }

    #[test]
    fn merge_proxy_configs_respects_overlay_precedence_and_ipv6_hosts() {
        let os_cfg = SystemProxyConfig {
            http_proxy: Some("http://127.0.0.1:7890".to_string()),
            https_proxy: Some("http://127.0.0.1:7890".to_string()),
            all_proxy: Some("socks5h://127.0.0.1:7891".to_string()),
            no_proxy: vec!["*.local".to_string()],
            exclude_simple_hostnames: true,
        };
        let app_override = SystemProxyConfig::from_all_proxy_url("socks5h://[::1]:10808");
        let merged = merge_proxy_configs(os_cfg, app_override);
        let target = reqwest::Url::parse("https://oauth2.googleapis.com/token").unwrap();
        assert_eq!(
            resolve_proxy_url_for_target(&merged, &target, true).as_deref(),
            Some("socks5h://[::1]:10808")
        );
        assert!(merged.exclude_simple_hostnames);
        assert_eq!(merged.no_proxy, vec!["*.local"]);
        assert_eq!(
            format_proxy_endpoint("socks5h", "::1", Some(1080)).as_deref(),
            Some("socks5h://[::1]:1080")
        );
    }
}
