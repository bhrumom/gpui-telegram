//! Host-managed MCP OAuth 2.1 client (`BotToolOAuth`):
//! - PKCE S256 (RFC 7636) with 32-byte CSPRNG verifier
//! - Dynamic Client Registration (RFC 7591)
//! - Resource Indicators (RFC 8707) on auth URL, token exchange, and token refresh
//! - Strict byte-for-byte exact-string Issuer verification (RFC 9207)
//! - Token Revocation (RFC 7009) with 5s deadline (`REVOKE_DEADLINE_SECS = 5`)
//! - SSRF private/loopback/link-local/CGNAT/ULA/multicast/metadata IP guard with DNS pinning
//!   and disabled redirects
//! - Proactive token refresh within 60s of expiry (`REFRESH_MARGIN_SECS = 60`),
//!   5-minute attempt TTL (`ATTEMPT_TTL_SECS = 300`), and 15s token deadline (`TOKEN_DEADLINE_SECS = 15`).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use url::{Host, Url};

use crate::{OrcaBotsError, Result, now_epoch_ms};

/// Proactive refresh margin before token expiration (60 seconds).
pub const REFRESH_MARGIN_SECS: u64 = 60;
/// Maximum lifetime of an in-flight OAuth authorization attempt (5 minutes).
pub const ATTEMPT_TTL_SECS: u64 = 300;
/// HTTP deadline for OAuth token exchange, refresh, and dynamic registration (15 seconds).
pub const TOKEN_DEADLINE_SECS: u64 = 15;
/// HTTP deadline for RFC 7009 token revocation requests (5 seconds).
pub const REVOKE_DEADLINE_SECS: u64 = 5;

const PKCE_VERIFIER_RANDOM_BYTES: usize = 32;
const OAUTH_STATE_RANDOM_BYTES: usize = 32;

/// SSRF enforcement policy for outbound OAuth endpoint connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthSecurityPolicy {
    allow_loopback_http: bool,
}

impl Default for OAuthSecurityPolicy {
    fn default() -> Self {
        Self::strict()
    }
}

impl OAuthSecurityPolicy {
    /// Production default: requires `https://` and blocks ALL private, loopback,
    /// link-local, CGNAT, ULA, unspecified, multicast, and cloud metadata IPs.
    pub const fn strict() -> Self {
        Self {
            allow_loopback_http: false,
        }
    }

    /// Test-only policy permitting `http://127.0.0.1:<port>` and `http://[::1]:<port>`
    /// loopback mock OAuth servers while still rejecting all RFC1918, link-local,
    /// CGNAT, ULA, unspecified, multicast, and metadata IP ranges.
    pub const fn allow_loopback_for_tests() -> Self {
        Self {
            allow_loopback_http: true,
        }
    }

    pub const fn allows_loopback(self) -> bool {
        self.allow_loopback_http
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr, allow_loopback: bool) -> bool {
    let [a, b, c, _d] = ip.octets();
    if ip.is_loopback() {
        return !allow_loopback;
    }
    if ip.is_unspecified()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
    {
        return true;
    }
    // 0.0.0.0/8 ("this network")
    if a == 0 {
        return true;
    }
    // 100.64.0.0/10 (Carrier-Grade NAT RFC 6598)
    if a == 100 && (64..=127).contains(&b) {
        return true;
    }
    // 192.0.0.0/24 (IETF protocol assignments)
    if a == 192 && b == 0 && c == 0 {
        return true;
    }
    // 198.18.0.0/15 (Benchmarking)
    if a == 198 && (b == 18 || b == 19) {
        return true;
    }
    // 240.0.0.0/4 (Reserved / future use)
    if a >= 240 {
        return true;
    }
    false
}

fn is_blocked_ipv6(ip: Ipv6Addr, allow_loopback: bool) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4, allow_loopback);
    }
    if ip.is_loopback() {
        return !allow_loopback;
    }
    if ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    let segs = ip.segments();
    // Unique Local Addresses: fc00::/7
    if (segs[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // Link-Local Unicast: fe80::/10
    if (segs[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // Site-Local (deprecated): fec0::/10
    if (segs[0] & 0xffc0) == 0xfec0 {
        return true;
    }
    // Documentation: 2001:db8::/32
    if segs[0] == 0x2001 && segs[1] == 0x0db8 {
        return true;
    }
    // NAT64 well-known prefix: 64:ff9b::/96 (embedded IPv4 in last 32 bits)
    if segs[0] == 0x0064
        && segs[1] == 0xff9b
        && segs[2] == 0
        && segs[3] == 0
        && segs[4] == 0
        && segs[5] == 0
    {
        let embedded = Ipv4Addr::new(
            (segs[6] >> 8) as u8,
            (segs[6] & 0xff) as u8,
            (segs[7] >> 8) as u8,
            (segs[7] & 0xff) as u8,
        );
        return is_blocked_ipv4(embedded, allow_loopback);
    }
    false
}

/// Returns `true` when `ip` is disallowed by SSRF protection under `allow_loopback`.
pub fn is_blocked_ip(ip: IpAddr, allow_loopback: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4, allow_loopback),
        IpAddr::V6(v6) => is_blocked_ipv6(v6, allow_loopback),
    }
}

fn is_forbidden_hostname(host_lower: &str, allow_loopback: bool) -> bool {
    if host_lower == "localhost" || host_lower.ends_with(".localhost") {
        return !allow_loopback;
    }
    host_lower == "metadata.google.internal"
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".localdomain")
}

/// Validates an OAuth endpoint URL syntactically and against SSRF IP/scheme rules.
pub fn validate_oauth_endpoint_url(raw_url: &str, policy: OAuthSecurityPolicy) -> Result<Url> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return Err(OrcaBotsError::oauth(
            "invalid_endpoint_url",
            "OAuth endpoint URL must be 1..=2048 characters.",
        ));
    }
    let parsed = Url::parse(trimmed)?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(OrcaBotsError::oauth(
            "ssrf_userinfo_blocked",
            "OAuth endpoint URL must not include embedded credentials.",
        ));
    }
    let Some(host) = parsed.host() else {
        return Err(OrcaBotsError::oauth(
            "invalid_endpoint_url",
            "OAuth endpoint URL must include a host.",
        ));
    };

    let is_literal_loopback = match host {
        Host::Ipv4(v4) => v4.is_loopback(),
        Host::Ipv6(v6) => v6.is_loopback(),
        Host::Domain(dom) => {
            let lower = dom.to_ascii_lowercase();
            lower == "localhost" || lower.ends_with(".localhost")
        }
    };

    match parsed.scheme() {
        "https" => {}
        "http" if policy.allow_loopback_http && is_literal_loopback => {}
        other => {
            return Err(OrcaBotsError::oauth(
                "ssrf_scheme_blocked",
                format!("OAuth endpoint scheme '{other}' is blocked (HTTPS required)."),
            ));
        }
    }

    match host {
        Host::Ipv4(v4) => {
            if is_blocked_ipv4(v4, policy.allow_loopback_http) {
                return Err(OrcaBotsError::oauth(
                    "ssrf_ip_blocked",
                    format!("OAuth endpoint IP '{v4}' is blocked by SSRF guard."),
                ));
            }
        }
        Host::Ipv6(v6) => {
            if is_blocked_ipv6(v6, policy.allow_loopback_http) {
                return Err(OrcaBotsError::oauth(
                    "ssrf_ip_blocked",
                    format!("OAuth endpoint IP '{v6}' is blocked by SSRF guard."),
                ));
            }
        }
        Host::Domain(domain) => {
            let lower = domain.trim_end_matches('.').to_ascii_lowercase();
            if lower.is_empty() || is_forbidden_hostname(&lower, policy.allow_loopback_http) {
                return Err(OrcaBotsError::oauth(
                    "ssrf_host_blocked",
                    format!("OAuth endpoint host '{domain}' is blocked by SSRF guard."),
                ));
            }
        }
    }

    Ok(parsed)
}

/// Validates an OAuth endpoint URL AND resolves its DNS records, rejecting ANY
/// resolved IP that violates `policy` (preventing DNS rebinding to private/metadata IPs).
pub async fn resolve_and_validate_oauth_url(
    raw_url: &str,
    policy: OAuthSecurityPolicy,
) -> Result<(Url, Vec<SocketAddr>)> {
    let parsed = validate_oauth_endpoint_url(raw_url, policy)?;
    let port = parsed.port_or_known_default().ok_or_else(|| {
        OrcaBotsError::oauth("invalid_endpoint_url", "OAuth endpoint URL lacks a known port.")
    })?;
    let host_str = parsed.host_str().ok_or_else(|| {
        OrcaBotsError::oauth("invalid_endpoint_url", "OAuth endpoint URL lacks a host.")
    })?;

    let addrs: Vec<SocketAddr> = match parsed.host() {
        Some(Host::Ipv4(v4)) => vec![SocketAddr::new(IpAddr::V4(v4), port)],
        Some(Host::Ipv6(v6)) => vec![SocketAddr::new(IpAddr::V6(v6), port)],
        Some(Host::Domain(dom)) => {
            let resolved = tokio::net::lookup_host((dom, port))
                .await
                .map_err(|err| {
                    OrcaBotsError::oauth(
                        "dns_resolution_failed",
                        format!("Failed to resolve OAuth endpoint host '{host_str}': {err}"),
                    )
                })?;
            let list: Vec<SocketAddr> = resolved.collect();
            if list.is_empty() {
                return Err(OrcaBotsError::oauth(
                    "dns_resolution_failed",
                    format!("OAuth endpoint host '{host_str}' resolved to zero addresses."),
                ));
            }
            list
        }
        None => {
            return Err(OrcaBotsError::oauth(
                "invalid_endpoint_url",
                "OAuth endpoint URL lacks a host.",
            ));
        }
    };

    for addr in &addrs {
        if is_blocked_ip(addr.ip(), policy.allow_loopback_http) {
            return Err(OrcaBotsError::oauth(
                "ssrf_ip_blocked",
                format!(
                    "OAuth endpoint host '{host_str}' resolved to blocked IP '{}'.",
                    addr.ip()
                ),
            ));
        }
    }

    Ok((parsed, addrs))
}

fn build_pinned_oauth_client(
    url: &Url,
    addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let Some(Host::Domain(domain)) = url.host() {
        builder = builder.resolve_to_addrs(domain, addrs);
    }
    builder.build().map_err(OrcaBotsError::Http)
}

fn random_base64url(byte_len: usize) -> Result<String> {
    let mut buf = vec![0u8; byte_len];
    getrandom::getrandom(&mut buf).map_err(|err| {
        OrcaBotsError::oauth("csprng_failed", format!("CSPRNG failed: {err}"))
    })?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// RFC 7636 PKCE S256 verifier/challenge pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

/// Generates a CSPRNG 32-byte PKCE verifier (base64url no-pad) and its SHA-256 `S256` challenge.
pub fn generate_pkce_pair() -> Result<PkcePair> {
    let code_verifier = random_base64url(PKCE_VERIFIER_RANDOM_BYTES)?;
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    Ok(PkcePair {
        code_verifier,
        code_challenge,
        code_challenge_method: "S256".to_string(),
    })
}

/// Validates RFC 8707 Resource Indicator URI (absolute URI, no fragment).
pub fn validate_resource_indicator(resource: &str) -> Result<String> {
    let trimmed = resource.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return Err(OrcaBotsError::oauth(
            "invalid_resource_indicator",
            "RFC 8707 resource indicator must be a non-empty URI.",
        ));
    }
    let parsed = Url::parse(trimmed)?;
    if parsed.fragment().is_some() {
        return Err(OrcaBotsError::oauth(
            "invalid_resource_indicator",
            "RFC 8707 resource indicator must not contain a fragment.",
        ));
    }
    Ok(trimmed.to_string())
}

/// RFC 7591 Dynamic Client Registration request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicClientRegistrationRequest {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    #[serde(default = "default_dcr_grant_types")]
    pub grant_types: Vec<String>,
    #[serde(default = "default_dcr_response_types")]
    pub response_types: Vec<String>,
    #[serde(default = "default_dcr_auth_method")]
    pub token_endpoint_auth_method: String,
}

fn default_dcr_grant_types() -> Vec<String> {
    vec![
        "authorization_code".to_string(),
        "refresh_token".to_string(),
    ]
}

fn default_dcr_response_types() -> Vec<String> {
    vec!["code".to_string()]
}

fn default_dcr_auth_method() -> String {
    "none".to_string()
}

/// RFC 7591 Dynamic Client Registration response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicClientRegistrationResponse {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<i64>,
}

/// OAuth 2.1 provider endpoints and client binding for a single MCP tool credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub resource: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Result returned when starting an OAuth 2.1 authorization attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthAuthorizationStart {
    pub state: String,
    pub authorization_url: String,
    pub code_challenge: String,
    pub expires_at_ms: i64,
}

/// Callback query parameters received at the OAuth redirect endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallbackParams {
    pub code: String,
    pub state: String,
    /// RFC 9207 `iss` parameter returned by the authorization server (exact-string checked).
    pub iss: Option<String>,
}

/// Host-stored OAuth token record (NEVER persisted inside bot home directories).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenRecord {
    pub credential_id: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub expires_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub resource: String,
}

#[derive(Debug, Clone)]
struct PendingAuthAttempt {
    credential_id: String,
    config: OAuthProviderConfig,
    code_verifier: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone)]
struct StoredCredentialEntry {
    config: OAuthProviderConfig,
    token: OAuthTokenRecord,
}

#[derive(Debug, Deserialize)]
struct RawTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

/// Host-side MCP OAuth 2.1 manager keeping tokens strictly outside bot home folders.
#[derive(Clone)]
pub struct BotToolOAuth {
    policy: OAuthSecurityPolicy,
    attempts: Arc<Mutex<HashMap<String, PendingAuthAttempt>>>,
    credentials: Arc<Mutex<HashMap<String, StoredCredentialEntry>>>,
    refresh_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    attempt_ttl_secs: u64,
    token_deadline_secs: u64,
    revoke_deadline_secs: u64,
}

impl std::fmt::Debug for BotToolOAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotToolOAuth")
            .field("policy", &self.policy)
            .field("attempt_ttl_secs", &self.attempt_ttl_secs)
            .field("token_deadline_secs", &self.token_deadline_secs)
            .field("revoke_deadline_secs", &self.revoke_deadline_secs)
            .finish()
    }
}

impl Default for BotToolOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl BotToolOAuth {
    /// Creates an OAuth 2.1 manager using `OAuthSecurityPolicy::strict()`.
    pub fn new() -> Self {
        Self::with_policy(OAuthSecurityPolicy::strict())
    }

    /// Creates an OAuth 2.1 manager with an explicit security policy.
    pub fn with_policy(policy: OAuthSecurityPolicy) -> Self {
        Self {
            policy,
            attempts: Arc::new(Mutex::new(HashMap::new())),
            credentials: Arc::new(Mutex::new(HashMap::new())),
            refresh_locks: Arc::new(Mutex::new(HashMap::new())),
            attempt_ttl_secs: ATTEMPT_TTL_SECS,
            token_deadline_secs: TOKEN_DEADLINE_SECS,
            revoke_deadline_secs: REVOKE_DEADLINE_SECS,
        }
    }

    /// Overrides the attempt TTL (used in tests to verify attempt expiration deterministically).
    pub fn with_attempt_ttl_secs(mut self, secs: u64) -> Self {
        self.attempt_ttl_secs = secs;
        self
    }

    /// Returns the active SSRF security policy.
    pub const fn policy(&self) -> OAuthSecurityPolicy {
        self.policy
    }

    fn refresh_lock_for(&self, credential_id: &str) -> Result<Arc<AsyncMutex<()>>> {
        let mut map = self.refresh_locks.lock().map_err(|_| {
            OrcaBotsError::oauth("lock_poisoned", "OAuth refresh lock table poisoned.")
        })?;
        Ok(map
            .entry(credential_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }

    /// Registers a client dynamically at an RFC 7591 registration endpoint with SSRF protection.
    pub async fn register_dynamic_client(
        &self,
        registration_endpoint: &str,
        req: &DynamicClientRegistrationRequest,
    ) -> Result<DynamicClientRegistrationResponse> {
        if req.client_name.trim().is_empty() || req.redirect_uris.is_empty() {
            return Err(OrcaBotsError::oauth(
                "invalid_dcr_request",
                "RFC 7591 DCR requires non-empty client_name and at least one redirect_uri.",
            ));
        }
        let (url, addrs) = resolve_and_validate_oauth_url(registration_endpoint, self.policy).await?;
        let client = build_pinned_oauth_client(
            &url,
            &addrs,
            Duration::from_secs(self.token_deadline_secs),
        )?;
        let resp = client
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(req)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(OrcaBotsError::oauth(
                "dcr_failed",
                format!("RFC 7591 registration failed with HTTP {}.", resp.status().as_u16()),
            ));
        }
        let parsed: DynamicClientRegistrationResponse = resp.json().await?;
        if parsed.client_id.trim().is_empty() {
            return Err(OrcaBotsError::oauth(
                "dcr_failed",
                "RFC 7591 registration response returned an empty client_id.",
            ));
        }
        Ok(parsed)
    }

    /// Starts an OAuth 2.1 + PKCE S256 authorization flow carrying RFC 8707 `resource`.
    pub fn begin_authorization(
        &self,
        credential_id: impl Into<String>,
        config: OAuthProviderConfig,
    ) -> Result<OAuthAuthorizationStart> {
        let credential_id = credential_id.into();
        if credential_id.trim().is_empty() || config.client_id.trim().is_empty() {
            return Err(OrcaBotsError::oauth(
                "invalid_config",
                "credential_id and client_id must be non-empty.",
            ));
        }
        if config.issuer.trim().is_empty() {
            return Err(OrcaBotsError::oauth(
                "invalid_config",
                "OAuth issuer must be non-empty for RFC 9207 verification.",
            ));
        }
        let mut auth_url = validate_oauth_endpoint_url(&config.authorization_endpoint, self.policy)?;
        validate_oauth_endpoint_url(&config.token_endpoint, self.policy)?;
        if let Some(ref rev) = config.revocation_endpoint {
            validate_oauth_endpoint_url(rev, self.policy)?;
        }
        let resource = validate_resource_indicator(&config.resource)?;

        let pkce = generate_pkce_pair()?;
        let state = random_base64url(OAUTH_STATE_RANDOM_BYTES)?;
        let now = now_epoch_ms();
        let expires_at_ms = now.saturating_add((self.attempt_ttl_secs as i64).saturating_mul(1000));

        {
            let mut pairs = auth_url.query_pairs_mut();
            pairs.append_pair("response_type", "code");
            pairs.append_pair("client_id", &config.client_id);
            pairs.append_pair("redirect_uri", &config.redirect_uri);
            pairs.append_pair("state", &state);
            pairs.append_pair("code_challenge", &pkce.code_challenge);
            pairs.append_pair("code_challenge_method", &pkce.code_challenge_method);
            pairs.append_pair("resource", &resource);
            if !config.scopes.is_empty() {
                pairs.append_pair("scope", &config.scopes.join(" "));
            }
        }

        let mut attempts = self.attempts.lock().map_err(|_| {
            OrcaBotsError::oauth("lock_poisoned", "OAuth attempt store mutex poisoned.")
        })?;
        attempts.retain(|_, v| v.expires_at_ms > now);
        attempts.insert(
            state.clone(),
            PendingAuthAttempt {
                credential_id,
                config: OAuthProviderConfig { resource, ..config },
                code_verifier: pkce.code_verifier,
                expires_at_ms,
            },
        );

        Ok(OAuthAuthorizationStart {
            state,
            authorization_url: auth_url.to_string(),
            code_challenge: pkce.code_challenge,
            expires_at_ms,
        })
    }

    /// Completes an OAuth 2.1 authorization callback:
    /// 1. Consumes & checks non-expired attempt by `state` (`ATTEMPT_TTL_SECS = 300`).
    /// 2. Enforces strict byte-for-byte RFC 9207 issuer equality (`callback.iss == expected_issuer`).
    /// 3. Exchanges `code` + PKCE `code_verifier` + RFC 8707 `resource` at `token_endpoint`
    ///    within `TOKEN_DEADLINE_SECS = 15` under SSRF protection.
    pub async fn complete_authorization(
        &self,
        callback: OAuthCallbackParams,
    ) -> Result<OAuthTokenRecord> {
        if callback.code.trim().is_empty() || callback.state.trim().is_empty() {
            return Err(OrcaBotsError::oauth(
                "invalid_callback",
                "OAuth callback requires non-empty code and state.",
            ));
        }

        let now = now_epoch_ms();
        let attempt = {
            let mut attempts = self.attempts.lock().map_err(|_| {
                OrcaBotsError::oauth("lock_poisoned", "OAuth attempt store mutex poisoned.")
            })?;
            attempts.remove(&callback.state)
        };
        let Some(attempt) = attempt else {
            return Err(OrcaBotsError::oauth(
                "invalid_state",
                "Unknown or already-consumed OAuth authorization state.",
            ));
        };
        if now > attempt.expires_at_ms {
            return Err(OrcaBotsError::oauth(
                "attempt_expired",
                "OAuth authorization attempt exceeded 5-minute TTL.",
            ));
        }

        // RFC 9207 exact-string issuer check (rejects trailing slash, case, or missing iss).
        let Some(cb_iss) = callback.iss.as_deref() else {
            return Err(OrcaBotsError::oauth(
                "issuer_missing",
                "OAuth callback is missing required RFC 9207 'iss' parameter.",
            ));
        };
        if cb_iss.as_bytes() != attempt.config.issuer.as_bytes() {
            return Err(OrcaBotsError::oauth(
                "issuer_mismatch",
                format!(
                    "RFC 9207 issuer mismatch: expected exact string '{}', got '{}'.",
                    attempt.config.issuer, cb_iss
                ),
            ));
        }

        let (token_url, addrs) =
            resolve_and_validate_oauth_url(&attempt.config.token_endpoint, self.policy).await?;
        let client = build_pinned_oauth_client(
            &token_url,
            &addrs,
            Duration::from_secs(self.token_deadline_secs),
        )?;

        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", &callback.code),
            ("redirect_uri", &attempt.config.redirect_uri),
            ("client_id", &attempt.config.client_id),
            ("code_verifier", &attempt.code_verifier),
            ("resource", &attempt.config.resource),
        ];
        if let Some(ref secret) = attempt.config.client_secret {
            form.push(("client_secret", secret.as_str()));
        }

        let resp = client
            .post(token_url)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(OrcaBotsError::oauth(
                "token_exchange_failed",
                format!(
                    "OAuth token exchange failed with HTTP {}.",
                    resp.status().as_u16()
                ),
            ));
        }
        let raw: RawTokenResponse = resp.json().await?;
        if raw.access_token.trim().is_empty() {
            return Err(OrcaBotsError::oauth(
                "token_exchange_failed",
                "OAuth token response returned an empty access_token.",
            ));
        }

        let issued_now = now_epoch_ms();
        let expires_in_secs = raw.expires_in.unwrap_or(3600);
        let record = OAuthTokenRecord {
            credential_id: attempt.credential_id.clone(),
            access_token: raw.access_token,
            refresh_token: raw.refresh_token,
            token_type: raw.token_type.unwrap_or_else(|| "Bearer".to_string()),
            expires_at_ms: issued_now.saturating_add((expires_in_secs as i64).saturating_mul(1000)),
            scope: raw.scope,
            resource: attempt.config.resource.clone(),
        };

        let mut creds = self.credentials.lock().map_err(|_| {
            OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
        })?;
        creds.insert(
            attempt.credential_id,
            StoredCredentialEntry {
                config: attempt.config,
                token: record.clone(),
            },
        );
        Ok(record)
    }

    /// Directly seeds a host-side OAuth token entry (useful for restored host vault state & tests).
    pub fn store_token_for_credential(
        &self,
        config: OAuthProviderConfig,
        token: OAuthTokenRecord,
    ) -> Result<()> {
        validate_oauth_endpoint_url(&config.token_endpoint, self.policy)?;
        validate_resource_indicator(&config.resource)?;
        let mut creds = self.credentials.lock().map_err(|_| {
            OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
        })?;
        creds.insert(
            token.credential_id.clone(),
            StoredCredentialEntry { config, token },
        );
        Ok(())
    }

    /// Reads the current host-side token record without mutating it.
    pub fn get_token_record(&self, credential_id: &str) -> Result<Option<OAuthTokenRecord>> {
        let creds = self.credentials.lock().map_err(|_| {
            OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
        })?;
        Ok(creds.get(credential_id).map(|e| e.token.clone()))
    }

    /// Returns a valid access token, proactively refreshing when within `REFRESH_MARGIN_SECS` (60s)
    /// of expiration.
    pub async fn get_valid_access_token(&self, credential_id: &str) -> Result<String> {
        let now = now_epoch_ms();
        let margin_ms = (REFRESH_MARGIN_SECS as i64).saturating_mul(1000);
        let maybe_fresh = {
            let creds = self.credentials.lock().map_err(|_| {
                OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
            })?;
            let Some(entry) = creds.get(credential_id) else {
                return Err(OrcaBotsError::oauth(
                    "credential_not_found",
                    format!("No OAuth token stored for credential '{credential_id}'."),
                ));
            };
            if entry.token.expires_at_ms > now.saturating_add(margin_ms) {
                Some(entry.token.access_token.clone())
            } else {
                None
            }
        };

        if let Some(token) = maybe_fresh {
            return Ok(token);
        }
        self.force_refresh(credential_id).await
    }

    /// Forces an OAuth 2.1 refresh-token exchange carrying RFC 8707 `resource` under a
    /// per-credential single-flight lock within `TOKEN_DEADLINE_SECS` (15s).
    pub async fn force_refresh(&self, credential_id: &str) -> Result<String> {
        let lock = self.refresh_lock_for(credential_id)?;
        let _guard = lock.lock().await;

        let entry = {
            let creds = self.credentials.lock().map_err(|_| {
                OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
            })?;
            creds.get(credential_id).cloned().ok_or_else(|| {
                OrcaBotsError::oauth(
                    "credential_not_found",
                    format!("No OAuth token stored for credential '{credential_id}'."),
                )
            })?
        };
        let Some(ref refresh_token) = entry.token.refresh_token else {
            return Err(OrcaBotsError::oauth(
                "sign_in_needed",
                format!("OAuth credential '{credential_id}' has no refresh_token."),
            ));
        };

        let (token_url, addrs) =
            resolve_and_validate_oauth_url(&entry.config.token_endpoint, self.policy).await?;
        let client = build_pinned_oauth_client(
            &token_url,
            &addrs,
            Duration::from_secs(self.token_deadline_secs),
        )?;

        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", &entry.config.client_id),
            ("resource", &entry.config.resource),
        ];
        if let Some(ref secret) = entry.config.client_secret {
            form.push(("client_secret", secret.as_str()));
        }

        let resp = client
            .post(token_url)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(OrcaBotsError::oauth(
                "refresh_failed",
                format!(
                    "OAuth token refresh failed with HTTP {}.",
                    resp.status().as_u16()
                ),
            ));
        }
        let raw: RawTokenResponse = resp.json().await?;
        if raw.access_token.trim().is_empty() {
            return Err(OrcaBotsError::oauth(
                "refresh_failed",
                "OAuth refresh response returned an empty access_token.",
            ));
        }

        let issued_now = now_epoch_ms();
        let expires_in_secs = raw.expires_in.unwrap_or(3600);
        let new_token = OAuthTokenRecord {
            credential_id: credential_id.to_string(),
            access_token: raw.access_token.clone(),
            refresh_token: raw.refresh_token.or_else(|| entry.token.refresh_token.clone()),
            token_type: raw.token_type.unwrap_or(entry.token.token_type),
            expires_at_ms: issued_now.saturating_add((expires_in_secs as i64).saturating_mul(1000)),
            scope: raw.scope.or(entry.token.scope),
            resource: entry.config.resource.clone(),
        };

        let mut creds = self.credentials.lock().map_err(|_| {
            OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
        })?;
        creds.insert(
            credential_id.to_string(),
            StoredCredentialEntry {
                config: entry.config,
                token: new_token,
            },
        );
        Ok(raw.access_token)
    }

    /// Sends an RFC 7009 token revocation request within `REVOKE_DEADLINE_SECS` (5s).
    pub async fn revoke_token(
        &self,
        revocation_endpoint: &str,
        client_id: &str,
        client_secret: Option<&str>,
        token: &str,
        token_type_hint: Option<&str>,
    ) -> Result<()> {
        let (revoke_url, addrs) =
            resolve_and_validate_oauth_url(revocation_endpoint, self.policy).await?;
        let client = build_pinned_oauth_client(
            &revoke_url,
            &addrs,
            Duration::from_secs(self.revoke_deadline_secs),
        )?;
        let mut form: Vec<(&str, &str)> = vec![("token", token), ("client_id", client_id)];
        if let Some(hint) = token_type_hint {
            form.push(("token_type_hint", hint));
        }
        if let Some(secret) = client_secret {
            form.push(("client_secret", secret));
        }
        let resp = client.post(revoke_url).form(&form).send().await?;
        if !resp.status().is_success() {
            return Err(OrcaBotsError::oauth(
                "revoke_failed",
                format!(
                    "RFC 7009 token revocation failed with HTTP {}.",
                    resp.status().as_u16()
                ),
            ));
        }
        Ok(())
    }

    /// Deletes a stored host credential immediately and attempts RFC 7009 revocation
    /// (preferring `refresh_token` if present, else `access_token`) within 5 seconds.
    pub async fn revoke_and_delete(&self, credential_id: &str) -> Result<bool> {
        let removed = {
            let mut creds = self.credentials.lock().map_err(|_| {
                OrcaBotsError::oauth("lock_poisoned", "OAuth credential store mutex poisoned.")
            })?;
            creds.remove(credential_id)
        };
        let Some(entry) = removed else {
            return Ok(false);
        };
        if let Some(ref endpoint) = entry.config.revocation_endpoint {
            let (tok, hint) = match entry.token.refresh_token.as_deref() {
                Some(rt) => (rt, Some("refresh_token")),
                None => (entry.token.access_token.as_str(), Some("access_token")),
            };
            self.revoke_token(
                endpoint,
                &entry.config.client_id,
                entry.config.client_secret.as_deref(),
                tok,
                hint,
            )
            .await?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn ssrf_guard_blocks_private_loopback_metadata_cgnat_ula_and_non_https() {
        let strict = OAuthSecurityPolicy::strict();
        let blocked_urls = [
            "http://auth.example.com/token",
            "https://user:pass@auth.example.com/token",
            "https://127.0.0.1/token",
            "https://[::1]/token",
            "https://localhost/token",
            "https://sub.localhost/token",
            "https://10.0.0.1/token",
            "https://172.16.5.4/token",
            "https://192.168.1.10/token",
            "https://169.254.169.254/latest/meta-data",
            "https://metadata.google.internal/computeMetadata/v1",
            "https://100.64.0.1/token",
            "https://0.0.0.0/token",
            "https://[::]/token",
            "https://[fc00::1]/token",
            "https://[fe80::1]/token",
            "https://[ff02::1]/token",
            "https://[::ffff:169.254.169.254]/token",
            "https://[::ffff:127.0.0.1]/token",
            "https://224.0.0.1/token",
        ];
        for raw in blocked_urls {
            assert!(
                validate_oauth_endpoint_url(raw, strict).is_err(),
                "strict policy MUST reject {raw}"
            );
        }
        assert!(validate_oauth_endpoint_url("https://auth.example.com/oauth/token", strict).is_ok());

        let test_policy = OAuthSecurityPolicy::allow_loopback_for_tests();
        assert!(validate_oauth_endpoint_url("http://127.0.0.1:8080/token", test_policy).is_ok());
        assert!(validate_oauth_endpoint_url("http://[::1]:8080/token", test_policy).is_ok());
        for still_blocked in [
            "https://10.0.0.1/token",
            "https://192.168.1.1/token",
            "https://169.254.169.254/token",
            "https://[fc00::1]/token",
            "http://example.com/token",
        ] {
            assert!(
                validate_oauth_endpoint_url(still_blocked, test_policy).is_err(),
                "test policy MUST still reject {still_blocked}"
            );
        }
    }

    async fn spawn_mock_oauth_server() -> (
        SocketAddr,
        Arc<Mutex<Vec<(String, String)>>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_requests: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let refresh_count = Arc::new(AtomicUsize::new(0));

        let seen_clone = seen_requests.clone();
        let refresh_clone = refresh_count.clone();
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let seen_c = seen_clone.clone();
                let refresh_c = refresh_clone.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let header_end;
                    loop {
                        let Ok(n) = stream.read(&mut tmp).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = pos + 4;
                            break;
                        }
                    }
                    let head_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let path = head_str
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let content_len: usize = head_str
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while buf.len() < header_end + content_len {
                        let Ok(n) = stream.read(&mut tmp).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body =
                        String::from_utf8_lossy(&buf[header_end..header_end + content_len])
                            .to_string();
                    seen_c.lock().unwrap().push((path.clone(), body.clone()));

                    let (status, resp_body) = if path == "/register" {
                        (
                            "201 Created",
                            r#"{"client_id":"dcr-client-1","client_secret":"dcr-sec-1"}"#
                                .to_string(),
                        )
                    } else if path == "/token" && body.contains("grant_type=authorization_code") {
                        (
                            "200 OK",
                            r#"{"access_token":"tok-initial","refresh_token":"ref-1","token_type":"Bearer","expires_in":30}"#
                                .to_string(),
                        )
                    } else if path == "/token" && body.contains("grant_type=refresh_token") {
                        let n = refresh_c.fetch_add(1, Ordering::SeqCst) + 1;
                        (
                            "200 OK",
                            format!(
                                r#"{{"access_token":"tok-refreshed-{n}","refresh_token":"ref-{n}","token_type":"Bearer","expires_in":3600}}"#
                            ),
                        )
                    } else if path == "/revoke" {
                        ("200 OK", "{}".to_string())
                    } else {
                        ("404 Not Found", r#"{"error":"unknown"}"#.to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        (addr, seen_requests, refresh_count, handle)
    }

    #[tokio::test]
    async fn oauth21_pkce_dcr_rfc8707_rfc9207_proactive_refresh_and_rfc7009_revoke() {
        let (addr, seen, refresh_count, server_handle) = spawn_mock_oauth_server().await;
        let base = format!("http://{addr}");
        let oauth = BotToolOAuth::with_policy(OAuthSecurityPolicy::allow_loopback_for_tests());

        // 1. Dynamic Client Registration (RFC 7591)
        let dcr = oauth
            .register_dynamic_client(
                &format!("{base}/register"),
                &DynamicClientRegistrationRequest {
                    client_name: "Orca Bot Tool".into(),
                    redirect_uris: vec!["http://127.0.0.1:43210/cb".into()],
                    grant_types: default_dcr_grant_types(),
                    response_types: default_dcr_response_types(),
                    token_endpoint_auth_method: "client_secret_post".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(dcr.client_id, "dcr-client-1");

        let resource = "https://mcp.example.com/v1";
        let cfg = OAuthProviderConfig {
            issuer: base.clone(),
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            revocation_endpoint: Some(format!("{base}/revoke")),
            registration_endpoint: Some(format!("{base}/register")),
            client_id: dcr.client_id,
            client_secret: dcr.client_secret,
            redirect_uri: "http://127.0.0.1:43210/cb".into(),
            resource: resource.into(),
            scopes: vec!["mcp:read".into()],
        };

        // 2. RFC 9207 exact-string issuer check rejects trailing slash & uppercase mismatch.
        let s_slash = oauth.begin_authorization("cred-slash", cfg.clone()).unwrap();
        let err_slash = oauth
            .complete_authorization(OAuthCallbackParams {
                code: "code-1".into(),
                state: s_slash.state,
                iss: Some(format!("{base}/")),
            })
            .await
            .unwrap_err();
        assert!(err_slash.to_string().contains("issuer_mismatch"));

        let s_case = oauth.begin_authorization("cred-case", cfg.clone()).unwrap();
        let err_case = oauth
            .complete_authorization(OAuthCallbackParams {
                code: "code-1".into(),
                state: s_case.state,
                iss: Some(base.to_uppercase()),
            })
            .await
            .unwrap_err();
        assert!(err_case.to_string().contains("issuer_mismatch"));

        // 3. Successful flow verifying PKCE S256 & RFC 8707 resource on auth URL + exchange.
        let started = oauth.begin_authorization("cred-main", cfg).unwrap();
        let parsed_auth = Url::parse(&started.authorization_url).unwrap();
        let q: HashMap<String, String> = parsed_auth.query_pairs().into_owned().collect();
        assert_eq!(q.get("code_challenge_method").map(String::as_str), Some("S256"));
        assert_eq!(q.get("code_challenge").map(String::as_str), Some(started.code_challenge.as_str()));
        assert_eq!(q.get("resource").map(String::as_str), Some(resource));

        let record = oauth
            .complete_authorization(OAuthCallbackParams {
                code: "auth-code-99".into(),
                state: started.state,
                iss: Some(base.clone()),
            })
            .await
            .unwrap();
        assert_eq!(record.access_token, "tok-initial");

        // Verify token exchange sent PKCE code_verifier matching S256 challenge AND RFC 8707 resource!
        let exchange_body = seen
            .lock()
            .unwrap()
            .iter()
            .find(|(p, b)| p == "/token" && b.contains("authorization_code"))
            .map(|(_, b)| b.clone())
            .unwrap();
        let ex_form: HashMap<String, String> = url::form_urlencoded::parse(exchange_body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(ex_form.get("resource").map(String::as_str), Some(resource));
        let verifier = ex_form.get("code_verifier").unwrap();
        let expected_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(expected_challenge, started.code_challenge);

        // 4. Token expires in 30s (< REFRESH_MARGIN_SECS = 60s), so get_valid_access_token proactively refreshes!
        let valid_tok = oauth.get_valid_access_token("cred-main").await.unwrap();
        assert_eq!(valid_tok, "tok-refreshed-1");
        assert_eq!(refresh_count.load(Ordering::SeqCst), 1);

        let refresh_body = seen
            .lock()
            .unwrap()
            .iter()
            .find(|(p, b)| p == "/token" && b.contains("refresh_token"))
            .map(|(_, b)| b.clone())
            .unwrap();
        let ref_form: HashMap<String, String> = url::form_urlencoded::parse(refresh_body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(ref_form.get("resource").map(String::as_str), Some(resource));

        // 5. RFC 7009 revoke_and_delete removes local token & hits /revoke.
        assert!(oauth.revoke_and_delete("cred-main").await.unwrap());
        assert!(oauth.get_token_record("cred-main").unwrap().is_none());
        assert!(seen.lock().unwrap().iter().any(|(p, _)| p == "/revoke"));

        server_handle.abort();
    }
}
