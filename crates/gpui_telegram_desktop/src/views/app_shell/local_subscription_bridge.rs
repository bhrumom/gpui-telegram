//! Client-side local AI subscription discovery & server bridge (Plan C architecture).
//!
//! Runs `magpie_gateway::discover_syncable_subscriptions_in_home` directly on the user's desktop
//! machine (macOS / Windows / Linux) so local credentials (`~/.config/gcloud`, `~/.config/magpie`,
//! `~/.codex`, `~/.claude`, `~/.gemini`, GitHub Copilot, Antigravity, Grok) are discovered even
//! when the desktop client is connected to a remote cloud server (e.g. `34.46.77.133:8080`).
//!
//! Dual-mode sync ensures instant out-of-the-box functionality:
//! 1. **Native sync (v1.3.3+ server)**: `POST /api/v1/hermes/magpie/subscriptions/sync` uploads the
//!    discovered subscription bundles + OAuth refresh credentials so the server registers native
//!    `VertexTokenManager` / `CodexTokenManager` instances and persists them cleanly.
//! 2. **Graceful legacy fallback (v1.3.0 servers)**: If `/subscriptions/sync` returns 404/405, the
//!    desktop client refreshes the OAuth2 access token locally via `VertexTokenManager` /
//!    `CodexTokenManager` and registers/activates the provider through the existing
//!    `POST /api/v1/hermes/magpie/providers` endpoint, while keeping all locally discovered cards
//!    visible in the UI and maintaining a background token keep-alive refresher.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bot_browser::e2ee_sync::AiOauthVaultBundle;
use chrono::Utc;
use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::{
    classify_provider_auth_error_code, format_provider_unavailable_reason,
    sanitize_router_state_for_cross_device, ApiMode, ProviderRouter, ProviderRouterState,
};
use gpui_telegram_client::net::hermes_client::{
    MagpieProbeRequest, MagpieRefreshProviderRequest, MagpieSaveProviderRequest, MagpieStateDto,
};
use gpui_telegram_client::TelegramClient;
use magpie_gateway::{
    default_models_for_subscription_kind, discover_syncable_subscriptions_in_home,
    extract_jwt_exp_ms, fetch_live_upstream_models, fetch_models_dev_catalog,
    is_cross_device_syncable_subscription, is_loopback_server_url, merge_discovered_models,
    ClientSubscriptionSyncItem, CodexTokenManager, CompactModelsCatalog, SubscriptionKind,
    VertexTokenManager, DEFAULT_TOKEN_REFRESH_LEAD_TIME_MS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::views::settings::sections::settings_ai_providers::{
    SubscriptionAccountDto, SubscriptionActivateRequest, SubscriptionActivateResponse,
};

/// Verified local availability status for a discovered subscription item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSubscriptionHealth {
    pub usable: bool,
    pub error_code: Option<String>,
    pub reason: Option<String>,
}

impl LocalSubscriptionHealth {
    pub fn usable() -> Self {
        Self {
            usable: true,
            error_code: None,
            reason: None,
        }
    }

    pub fn unavailable(error_code: impl Into<String>, reason: impl Into<String>) -> Self {
        let code = error_code.into();
        let msg = reason.into();
        Self {
            usable: false,
            error_code: (!code.trim().is_empty()).then_some(code),
            reason: (!msg.trim().is_empty()).then_some(msg),
        }
    }
}

/// Active interactive browser/CLI re-authentication session for a provider/subscription.
#[derive(Debug)]
pub struct ActiveProviderReauth {
    pub provider_id: String,
    pub display_name: String,
    pub status_zh: String,
    pub status_en: String,
    pub generation: u64,
    pub cancel_tx: tokio::sync::watch::Sender<bool>,
}

/// Per-`AppShell` state for client-side local AI subscription discovery, re-auth, and legacy token keep-alive.
#[derive(Debug, Default)]
pub struct ShellMagpieLocalState {
    /// Cached client-discovered local subscription bundles (secrets redacted in `Debug`).
    pub discovered_items: Vec<ClientSubscriptionSyncItem>,
    /// Verified local availability per discovered subscription ID.
    pub discovered_health: HashMap<String, LocalSubscriptionHealth>,
    /// Latest live `MagpieStateDto` fetched from Settings/Magpie backend for cross-view sync.
    pub latest_state: Option<MagpieStateDto>,
    /// Latest server-fetched subscription DTOs for cross-view model catalog sync.
    pub server_subscriptions: Vec<SubscriptionAccountDto>,
    /// Auth generation for which automatic startup bootstrap has already been dispatched.
    pub bootstrapped_auth_generation: Option<u64>,
    /// Generation counter for cancelling stale legacy keep-alive loops.
    pub legacy_keepalive_generation: u64,
    /// Currently active legacy-synced subscription ID (when connected to a v1.3.0 server).
    pub active_legacy_subscription_id: Option<String>,
    /// Currently active model override for the legacy-synced subscription.
    pub active_legacy_model: Option<String>,
    /// Currently running interactive browser/CLI re-authentication session, if any.
    pub reauth_in_progress: Option<ActiveProviderReauth>,
    /// Generation counter for interactive re-authentication sessions.
    pub reauth_generation: u64,
    /// Provider/subscription IDs currently executing a force refresh & re-activate check.
    pub refreshing_provider_ids: HashSet<String>,
}

impl ShellMagpieLocalState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel_active_reauth(&mut self) -> bool {
        self.reauth_generation = self.reauth_generation.wrapping_add(1);
        if let Some(active) = self.reauth_in_progress.take() {
            let _ = active.cancel_tx.send(true);
            true
        } else {
            false
        }
    }

    pub fn reset_on_logout(&mut self) {
        self.cancel_active_reauth();
        self.discovered_items.clear();
        self.discovered_health.clear();
        self.latest_state = None;
        self.server_subscriptions.clear();
        self.bootstrapped_auth_generation = None;
        self.legacy_keepalive_generation = self.legacy_keepalive_generation.wrapping_add(1);
        self.active_legacy_subscription_id = None;
        self.active_legacy_model = None;
        self.refreshing_provider_ids.clear();
    }

    pub fn is_local_item_usable(&self, id: &str) -> bool {
        self.discovered_health.get(id).is_none_or(|h| h.usable)
    }

    pub fn cached_dtos(&self, active_provider: Option<&str>) -> Vec<SubscriptionAccountDto> {
        self.discovered_items
            .iter()
            .map(|item| {
                sync_item_to_account_dto_with_health(
                    item,
                    active_provider,
                    self.discovered_health.get(&item.account.id),
                )
            })
            .collect()
    }
}

/// Resolves the current desktop user's home directory across macOS, Linux, and Windows.
pub fn resolve_local_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()))
        .map(PathBuf::from)
}

/// Runs read-only discovery of all local AI subscriptions on the user's desktop machine.
pub fn discover_local_subscriptions_for_server(server_base_url: &str) -> Vec<ClientSubscriptionSyncItem> {
    if std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
        return Vec::new();
    }
    let Some(home) = resolve_local_home_dir() else {
        return Vec::new();
    };
    let codex_override = std::env::var_os("CODEX_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let target_is_remote = !is_loopback_server_url(server_base_url);
    discover_syncable_subscriptions_in_home(&home, codex_override.as_deref(), target_is_remote)
}

/// Converts a client-discovered `ClientSubscriptionSyncItem` into a UI `SubscriptionAccountDto`.
pub fn sync_item_to_account_dto(
    item: &ClientSubscriptionSyncItem,
    active_provider: Option<&str>,
) -> SubscriptionAccountDto {
    sync_item_to_account_dto_with_health(item, active_provider, None)
}

/// Converts a client-discovered `ClientSubscriptionSyncItem` into a UI `SubscriptionAccountDto`,
/// incorporating verified local health (`LocalSubscriptionHealth`) when available.
pub fn sync_item_to_account_dto_with_health(
    item: &ClientSubscriptionSyncItem,
    active_provider: Option<&str>,
    health: Option<&LocalSubscriptionHealth>,
) -> SubscriptionAccountDto {
    let acc = &item.account;
    let mut models = if !item.available_models.is_empty() {
        item.available_models.clone()
    } else {
        default_models_for_subscription_kind(acc.kind)
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    };
    if let Some(ref dm) = acc.default_model {
        let trimmed = dm.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with("codex/")
            && !trimmed.starts_with("auto/")
            && !models.iter().any(|m| m == trimmed)
        {
            models.insert(0, trimmed.to_string());
        }
    }

    let now_ms = Utc::now().timestamp_millis();
    let (mut usable, mut unavailable_code, mut unavailable_reason) = health
        .map(|h| (h.usable, h.error_code.clone(), h.reason.clone()))
        .unwrap_or((true, None, None));
    if usable {
        let non_refreshable =
            acc.kind != SubscriptionKind::GoogleVertex && acc.kind != SubscriptionKind::CodexChatGpt;
        if non_refreshable && acc.expires_at_ms.is_some_and(|exp| exp <= now_ms) {
            usable = false;
            unavailable_code = Some("token_expired".to_string());
            unavailable_reason = Some("登录凭据已过期，请点击「重新登录」".to_string());
        } else if !acc.active
            && item.vertex_adc.is_none()
            && item.codex_oauth.is_none()
            && item
                .access_token
                .as_deref()
                .or(acc.access_token.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none()
        {
            usable = false;
            unavailable_code = Some("missing_credentials".to_string());
            unavailable_reason = Some("未检测到有效凭据，请点击「重新登录」".to_string());
        }
    }

    let active = usable
        && active_provider.is_some_and(|ap| {
            ap == acc.id
                || (acc.kind == SubscriptionKind::GoogleVertex && ap == "magpie_provider:google-vertex")
        });

    SubscriptionAccountDto {
        id: acc.id.clone(),
        account_id: acc.account_id.clone().or_else(|| acc.email.clone()),
        kind: acc.kind.as_str().to_string(),
        label: acc.label.clone(),
        plan: acc.plan.clone(),
        models,
        active,
        usable,
        unavailable_code,
        unavailable_reason,
    }
}

fn vertex_token_url_from_item(item: &ClientSubscriptionSyncItem) -> Option<String> {
    item.raw_headers
        .iter()
        .chain(item.account.headers.iter())
        .find(|(k, _)| k.eq_ignore_ascii_case("x-vertex-token-url"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn is_local_offline_test_mode() -> bool {
    if std::env::var_os("HERMES_FORCE_LIVE_HEALTH_CHECK").is_some() {
        return false;
    }
    if cfg!(test)
        || std::env::var_os("GPUI_OFFLINE_TEST").is_some()
        || std::env::var_os("NEXTEST").is_some()
        || std::env::var_os("NEXTEST_RUN_ID").is_some()
    {
        return true;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let parent_dir = exe.parent()?;
            let parent = parent_dir.file_name()?.to_str()?;
            let stem = exe.file_stem()?.to_str()?;
            let in_deps = parent == "deps" && stem.contains('-');
            let has_hash_suffix = stem.rsplit_once('-').is_some_and(|(_, suffix)| {
                suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
            });
            let in_cargo_build_out = parent == "out"
                && has_hash_suffix
                && parent_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    == Some("build");
            Some(in_deps || in_cargo_build_out)
        })
        .unwrap_or(false)
}

async fn verify_single_discovered_subscription(
    http: &reqwest::Client,
    item: &ClientSubscriptionSyncItem,
) -> LocalSubscriptionHealth {
    let now_ms = Utc::now().timestamp_millis();
    let offline_test = is_local_offline_test_mode();
    let acc = &item.account;

    match acc.kind {
        SubscriptionKind::GoogleVertex => {
            if let Some(ref adc) = item.vertex_adc {
                if adc.refresh_token.trim().is_empty() || adc.client_id.trim().is_empty() {
                    return LocalSubscriptionHealth::unavailable(
                        "missing_credentials",
                        "Google Vertex AI ADC 凭据不完整，请点击「重新登录」完成授权",
                    );
                }
                let custom_token_url = vertex_token_url_from_item(item);
                let is_loopback_token_url = custom_token_url
                    .as_deref()
                    .is_some_and(is_loopback_server_url);
                if offline_test && !is_loopback_token_url {
                    return LocalSubscriptionHealth::usable();
                }
                let mut mgr = VertexTokenManager::new(adc.clone());
                if let Some(url) = custom_token_url {
                    mgr = mgr.with_token_url(url);
                }
                match tokio::time::timeout(
                    Duration::from_secs(8),
                    mgr.get_or_refresh_token(http, now_ms),
                )
                .await
                {
                    Ok(Ok(_)) => LocalSubscriptionHealth::usable(),
                    Ok(Err(e)) => {
                        let raw = e.to_string();
                        let code = classify_provider_auth_error_code(&raw);
                        LocalSubscriptionHealth::unavailable(
                            code,
                            format_provider_unavailable_reason(&acc.id, &raw),
                        )
                    }
                    Err(_) => LocalSubscriptionHealth::unavailable(
                        "unreachable",
                        "Google Vertex AI OAuth 验证超时，请检查网络连接",
                    ),
                }
            } else if let Some(tok) = item
                .access_token
                .as_deref()
                .or(acc.access_token.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let expired = acc.expires_at_ms.is_some_and(|exp| exp <= now_ms)
                    || extract_jwt_exp_ms(tok).is_some_and(|exp| exp <= now_ms);
                if expired {
                    LocalSubscriptionHealth::unavailable(
                        "token_expired",
                        "Google Vertex AI 访问令牌已过期，请点击「重新登录」",
                    )
                } else {
                    LocalSubscriptionHealth::usable()
                }
            } else {
                LocalSubscriptionHealth::unavailable(
                    "missing_credentials",
                    "缺少 Google Vertex AI 凭据，请点击「重新登录」",
                )
            }
        }
        SubscriptionKind::CodexChatGpt => {
            let access_tok = item
                .access_token
                .as_deref()
                .or(acc.access_token.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(tok) = access_tok {
                let is_fresh = extract_jwt_exp_ms(tok).is_none_or(|exp| {
                    exp > now_ms.saturating_add(DEFAULT_TOKEN_REFRESH_LEAD_TIME_MS)
                });
                if is_fresh && acc.expires_at_ms.is_none_or(|exp| exp > now_ms) {
                    return LocalSubscriptionHealth::usable();
                }
            }
            if let Some(ref oauth) = item.codex_oauth {
                if !oauth.refresh_token.trim().is_empty() {
                    let is_loopback_token_url = oauth
                        .token_url
                        .as_deref()
                        .is_some_and(is_loopback_server_url);
                    if offline_test && !is_loopback_token_url {
                        return LocalSubscriptionHealth::usable();
                    }
                    let mut mgr = CodexTokenManager::new(oauth.refresh_token.clone());
                    if let Some(ref cid) = oauth.client_id {
                        if !cid.trim().is_empty() {
                            mgr = mgr.with_client_id(cid.clone());
                        }
                    }
                    if let Some(ref url) = oauth.token_url {
                        if !url.trim().is_empty() {
                            mgr = mgr.with_token_url(url.clone());
                        }
                    }
                    if acc.source_path.ends_with("auth.json") {
                        mgr = mgr.with_persist_path(PathBuf::from(&acc.source_path));
                    }
                    return match tokio::time::timeout(
                        Duration::from_secs(8),
                        mgr.get_or_refresh_token(http, now_ms),
                    )
                    .await
                    {
                        Ok(Ok(_)) => LocalSubscriptionHealth::usable(),
                        Ok(Err(e)) => {
                            let raw = e.to_string();
                            let code = classify_provider_auth_error_code(&raw);
                            LocalSubscriptionHealth::unavailable(
                                code,
                                format_provider_unavailable_reason(&acc.id, &raw),
                            )
                        }
                        Err(_) => LocalSubscriptionHealth::unavailable(
                            "unreachable",
                            "Codex / ChatGPT OAuth 验证超时，请检查网络连接",
                        ),
                    };
                }
            }
            if access_tok.is_some() {
                LocalSubscriptionHealth::unavailable(
                    "token_expired",
                    "Codex / ChatGPT OAuth 令牌已过期，请点击「重新登录」",
                )
            } else {
                LocalSubscriptionHealth::unavailable(
                    "missing_credentials",
                    "缺少 Codex / ChatGPT 登录凭据，请点击「重新登录」",
                )
            }
        }
        _ => {
            let tok = item
                .access_token
                .as_deref()
                .or(acc.access_token.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if tok.is_none() {
                return LocalSubscriptionHealth::unavailable(
                    "missing_credentials",
                    "未检测到有效凭据，请点击「重新登录」",
                );
            }
            if acc.expires_at_ms.is_some_and(|exp| exp <= now_ms)
                || tok.and_then(extract_jwt_exp_ms).is_some_and(|exp| exp <= now_ms)
            {
                return LocalSubscriptionHealth::unavailable(
                    "token_expired",
                    "登录凭据已过期，请点击「重新登录」",
                );
            }
            if let Some(base_url) = acc
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let is_loopback = is_loopback_server_url(base_url);
                if is_loopback {
                    let probe_url = format!("{}/models", base_url.trim_end_matches('/'));
                    let mut req = http.get(&probe_url);
                    if let Some(t) = tok {
                        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {t}"));
                    }
                    return match tokio::time::timeout(Duration::from_millis(1500), req.send()).await {
                        Ok(Ok(resp)) => {
                            let status = resp.status();
                            if status == reqwest::StatusCode::UNAUTHORIZED
                                || status == reqwest::StatusCode::FORBIDDEN
                            {
                                let msg = format!("HTTP {}", status.as_u16());
                                LocalSubscriptionHealth::unavailable(
                                    classify_provider_auth_error_code(&msg),
                                    format_provider_unavailable_reason(&acc.id, &msg),
                                )
                            } else {
                                LocalSubscriptionHealth::usable()
                            }
                        }
                        Ok(Err(_)) | Err(_) => LocalSubscriptionHealth::unavailable(
                            "unreachable",
                            "本地代理服务未运行或无法连接",
                        ),
                    };
                } else if !offline_test && acc.kind == SubscriptionKind::ClaudeCode {
                    let probe_url = format!("{}/models", base_url.trim_end_matches('/'));
                    let mut req = http
                        .get(&probe_url)
                        .header("anthropic-version", "2023-06-01");
                    if let Some(t) = tok {
                        if t.starts_with("sk-ant-oat") {
                            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {t}"));
                        } else {
                            req = req.header("x-api-key", t);
                        }
                    }
                    if let Ok(Ok(resp)) =
                        tokio::time::timeout(Duration::from_secs(5), req.send()).await
                    {
                        let status = resp.status();
                        if status == reqwest::StatusCode::UNAUTHORIZED
                            || status == reqwest::StatusCode::FORBIDDEN
                        {
                            let msg = format!("HTTP {}", status.as_u16());
                            return LocalSubscriptionHealth::unavailable(
                                classify_provider_auth_error_code(&msg),
                                format_provider_unavailable_reason(&acc.id, &msg),
                            );
                        }
                    }
                }
            }
            LocalSubscriptionHealth::usable()
        }
    }
}

/// Concurrently verifies whether each client-discovered local subscription is usable right now
/// (e.g., testing Google Vertex AI OAuth refresh token exchange to catch `invalid_grant` early).
pub async fn verify_discovered_subscriptions(
    items: &[ClientSubscriptionSyncItem],
) -> HashMap<String, LocalSubscriptionHealth> {
    if items.is_empty() {
        return HashMap::new();
    }
    magpie_gateway::invalidate_system_proxy_cache();
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(6)),
        None,
    );
    let mut join_set = tokio::task::JoinSet::new();
    for item in items.iter().cloned() {
        let http_cl = http.clone();
        join_set.spawn(async move {
            let id = item.account.id.clone();
            let status = verify_single_discovered_subscription(&http_cl, &item).await;
            (id, status)
        });
    }
    let mut out = HashMap::with_capacity(items.len());
    while let Some(res) = join_set.join_next().await {
        if let Ok((id, status)) = res {
            out.insert(id, status);
        }
    }
    out
}

/// Orders active locally discovered subscriptions by activation priority:
/// 1. Google Vertex AI ADC (`google_vertex:*` before `magpie_provider:*`)
/// 2. Codex / ChatGPT (`codex:*` or `magpie_codex:*`)
/// 3. Claude Code / Gemini Code Assist / others
pub fn pick_ordered_bootstrap_candidates(
    items: &[ClientSubscriptionSyncItem],
) -> Vec<ClientSubscriptionSyncItem> {
    let mut active: Vec<ClientSubscriptionSyncItem> =
        items.iter().filter(|i| i.account.active).cloned().collect();
    active.sort_by_key(|item| match item.account.kind {
        SubscriptionKind::GoogleVertex => {
            if item.account.id.starts_with("google_vertex:") {
                0
            } else {
                1
            }
        }
        SubscriptionKind::CodexChatGpt => {
            if item.account.id.starts_with("codex:") {
                2
            } else {
                3
            }
        }
        SubscriptionKind::ClaudeCode => 4,
        SubscriptionKind::GeminiCodeAssist => 5,
        SubscriptionKind::GitHubCopilot => 6,
        SubscriptionKind::Antigravity => 7,
        SubscriptionKind::GrokSuperGrok => 8,
    });
    active
}

/// Obtains a fresh bearer token locally for the given subscription item, refreshing Google Vertex
/// ADC or Codex OAuth2 credentials when needed.
pub async fn resolve_fresh_bearer_token(
    http_client: &reqwest::Client,
    item: &ClientSubscriptionSyncItem,
) -> Result<String, String> {
    let now_ms = Utc::now().timestamp_millis();

    match item.account.kind {
        SubscriptionKind::GoogleVertex => {
            if let Some(ref adc) = item.vertex_adc {
                let mut mgr = VertexTokenManager::new(adc.clone());
                if let Some(url) = vertex_token_url_from_item(item) {
                    mgr = mgr.with_token_url(url);
                }
                return mgr
                    .get_or_refresh_token(http_client, now_ms)
                    .await
                    .map_err(|e| format!("Vertex ADC token refresh failed: {e}"));
            }
            item.access_token
                .clone()
                .filter(|t| !t.trim().is_empty())
                .ok_or_else(|| "Missing Google Vertex ADC credentials".to_string())
        }
        SubscriptionKind::CodexChatGpt => {
            if let Some(ref tok) = item.access_token {
                let trimmed = tok.trim();
                if !trimmed.is_empty() {
                    let is_fresh = extract_jwt_exp_ms(trimmed).is_none_or(|exp| {
                        exp > now_ms.saturating_add(DEFAULT_TOKEN_REFRESH_LEAD_TIME_MS)
                    });
                    if is_fresh {
                        return Ok(trimmed.to_string());
                    }
                }
            }
            if let Some(ref oauth) = item.codex_oauth {
                let mut mgr = CodexTokenManager::new(oauth.refresh_token.clone());
                if let Some(ref cid) = oauth.client_id {
                    if !cid.trim().is_empty() {
                        mgr = mgr.with_client_id(cid.clone());
                    }
                }
                if let Some(ref url) = oauth.token_url {
                    if !url.trim().is_empty() {
                        mgr = mgr.with_token_url(url.clone());
                    }
                }
                return mgr
                    .get_or_refresh_token(http_client, now_ms)
                    .await
                    .map_err(|e| format!("Codex OAuth token refresh failed: {e}"));
            }
            item.access_token
                .clone()
                .filter(|t| !t.trim().is_empty())
                .ok_or_else(|| "Missing Codex OAuth access/refresh token".to_string())
        }
        _ => item
            .access_token
            .clone()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| format!("Subscription '{}' has no usable access token", item.account.id)),
    }
}

/// Builds a `MagpieSaveProviderRequest` compatible with both v1.3.0 and newer servers.
pub fn build_legacy_save_provider_request(
    item: &ClientSubscriptionSyncItem,
    bearer_token: String,
    model_override: Option<&str>,
    activate: bool,
) -> MagpieSaveProviderRequest {
    let acc = &item.account;
    let (api_mode, fallback_base_url, fallback_model) = match acc.kind {
        SubscriptionKind::GoogleVertex => (
            "google_vertex",
            "https://aiplatform.googleapis.com/v1",
            "gemini-2.5-flash",
        ),
        SubscriptionKind::CodexChatGpt => (
            "codex_responses",
            "https://chatgpt.com/backend-api/codex",
            "codex-mini-latest",
        ),
        SubscriptionKind::ClaudeCode => (
            "anthropic_messages",
            "https://api.anthropic.com/v1",
            "claude-sonnet-4-20250514",
        ),
        SubscriptionKind::GeminiCodeAssist => (
            "gemini_generate_content",
            "https://cloudcode-pa.googleapis.com/v1internal",
            "gemini-2.5-flash",
        ),
        _ => ("chat_completions", "https://api.openai.com/v1", "gpt-4o-mini"),
    };

    let base_url = acc
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_base_url)
        .to_string();

    let default_model = model_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            acc.default_model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(fallback_model)
        .to_string();

    let mut custom_headers = HashMap::new();
    for (k, v) in &item.raw_headers {
        let lower = k.to_ascii_lowercase();
        if !lower.starts_with("x-adc-") && !lower.starts_with("x-codex-") {
            custom_headers.insert(k.clone(), v.clone());
        }
    }
    if acc.kind == SubscriptionKind::GoogleVertex {
        if let Some(ref proj) = acc
            .account_id
            .clone()
            .or_else(|| item.vertex_adc.as_ref().and_then(|a| a.quota_project_id.clone()))
        {
            custom_headers
                .entry("x-goog-user-project".to_string())
                .or_insert_with(|| proj.clone());
        }
    } else if acc.kind == SubscriptionKind::CodexChatGpt {
        if let Some(ref acct) = acc.account_id {
            let has_acct = custom_headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("chatgpt-account-id"));
            if !has_acct {
                custom_headers.insert("chatgpt-account-id".to_string(), acct.clone());
            }
        }
    }

    MagpieSaveProviderRequest {
        preset_id: None,
        provider_id: Some(acc.id.clone()),
        name: Some(acc.label.clone()),
        region_id: item.region_id.clone(),
        api_key: Some(bearer_token),
        api_mode: Some(api_mode.to_string()),
        base_url: Some(base_url),
        default_model: Some(default_model),
        custom_headers,
        enabled: Some(true),
        no_key_required: Some(false),
        activate,
        allow_local_fallback: None,
    }
}

#[derive(Debug, Serialize)]
struct NativeSyncPayload<'a> {
    subscriptions: &'a [ClientSubscriptionSyncItem],
    auto_activate: bool,
}

#[derive(Debug, Deserialize)]
struct NativeSyncResponse {
    #[serde(default)]
    pub synced_count: usize,
    #[serde(default)]
    pub state: Option<MagpieStateDto>,
}

/// Outcome of syncing client-discovered local subscriptions to the backend server.
pub struct LocalSubscriptionSyncOutcome {
    pub state: Option<MagpieStateDto>,
    pub local_items: Vec<ClientSubscriptionSyncItem>,
    pub local_health: HashMap<String, LocalSubscriptionHealth>,
    pub used_legacy_fallback: bool,
    pub auto_activated_id: Option<String>,
    pub auto_activated_label: Option<String>,
    pub auto_activated_model: Option<String>,
    pub warning: Option<String>,
}

fn check_pinned_epoch(
    pinned: Option<(&TelegramClient, u64)>,
    token: &str,
) -> Result<(), String> {
    if let Some((client, epoch)) = pinned {
        client
            .ensure_pinned_session(epoch, token)
            .map_err(|e| format!("Session changed during subscription operation: {e}"))?;
    }
    Ok(())
}

/// Syncs client-discovered local subscriptions to the backend server with automatic fallback
/// for older v1.3.0 servers that do not expose `/api/v1/hermes/magpie/subscriptions/sync`.
pub async fn sync_local_subscriptions_to_server(
    client: &Arc<TelegramClient>,
    base_url: &str,
    token: &str,
    items: Vec<ClientSubscriptionSyncItem>,
    auto_activate: bool,
) -> LocalSubscriptionSyncOutcome {
    let items_fallback = items.clone();
    sync_local_subscriptions_to_server_inner(client, None, base_url, token, items, auto_activate)
        .await
        .unwrap_or_else(|e| LocalSubscriptionSyncOutcome {
            state: None,
            local_items: items_fallback,
            local_health: HashMap::new(),
            used_legacy_fallback: false,
            auto_activated_id: None,
            auto_activated_label: None,
            auto_activated_model: None,
            warning: Some(e),
        })
}

/// Session-epoch-pinned variant of [`sync_local_subscriptions_to_server`] enforcing
/// `ensure_pinned_session` before every HTTP request.
pub async fn sync_local_subscriptions_to_server_pinned(
    client: &Arc<TelegramClient>,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
    items: Vec<ClientSubscriptionSyncItem>,
    auto_activate: bool,
) -> Result<LocalSubscriptionSyncOutcome, String> {
    sync_local_subscriptions_to_server_inner(
        client,
        Some(expected_epoch),
        base_url,
        token,
        items,
        auto_activate,
    )
    .await
}

async fn sync_local_subscriptions_to_server_inner(
    client: &Arc<TelegramClient>,
    pinned_epoch: Option<u64>,
    base_url: &str,
    token: &str,
    items: Vec<ClientSubscriptionSyncItem>,
    auto_activate: bool,
) -> Result<LocalSubscriptionSyncOutcome, String> {
    let pinned = pinned_epoch.map(|e| (client.as_ref(), e));
    let target_is_remote = !is_loopback_server_url(base_url);
    let items: Vec<ClientSubscriptionSyncItem> = if target_is_remote {
        items
            .into_iter()
            .filter(is_cross_device_syncable_subscription)
            .collect()
    } else {
        items
    };
    let mut local_health = verify_discovered_subscriptions(&items).await;
    if items.is_empty() && !target_is_remote {
        check_pinned_epoch(pinned, token)?;
        let state = client.hermes.magpie_state(token).await.ok();
        return Ok(LocalSubscriptionSyncOutcome {
            state,
            local_items: items,
            local_health,
            used_legacy_fallback: false,
            auto_activated_id: None,
            auto_activated_label: None,
            auto_activated_model: None,
            warning: None,
        });
    }

    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(20)),
    );
    let sync_url = format!(
        "{}/api/v1/hermes/magpie/subscriptions/sync",
        base_url.trim_end_matches('/')
    );
    let payload = NativeSyncPayload {
        subscriptions: &items,
        auto_activate,
    };

    check_pinned_epoch(pinned, token)?;
    if let Ok(resp) = http
        .post(&sync_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&payload)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(parsed) = resp.json::<NativeSyncResponse>().await {
                let state = match parsed.state {
                    Some(st) => Some(st),
                    None => {
                        check_pinned_epoch(pinned, token)?;
                        client.hermes.magpie_state(token).await.ok()
                    }
                };
                if let Some(ref st) = state {
                    for s in &st.subscriptions {
                        if !s.is_selectable() {
                            local_health.insert(
                                s.id.clone(),
                                LocalSubscriptionHealth::unavailable(
                                    s.unavailable_code.clone().unwrap_or_default(),
                                    s.unavailable_reason.clone().unwrap_or_default(),
                                ),
                            );
                        } else if target_is_remote
                            && local_health.get(&s.id).is_some_and(|h| {
                                !h.usable && h.error_code.as_deref() == Some("unreachable")
                            })
                        {
                            local_health.insert(s.id.clone(), LocalSubscriptionHealth::usable());
                        }
                    }
                }
                let (act_id, act_label, act_model) = state
                    .as_ref()
                    .filter(|s| {
                        !s.needs_provider_selection
                            && !s.active_provider.is_empty()
                            && s.active_provider != "local_hermes"
                            && local_health
                                .get(&s.active_provider)
                                .is_none_or(|h| h.usable)
                    })
                    .map(|s| {
                        let lbl = items
                            .iter()
                            .find(|i| i.account.id == s.active_provider)
                            .map(|i| i.account.label.clone());
                        (
                            Some(s.active_provider.clone()),
                            lbl,
                            Some(s.active_model.clone()),
                        )
                    })
                    .unwrap_or((None, None, None));
                let _ = parsed.synced_count;
                return Ok(LocalSubscriptionSyncOutcome {
                    state,
                    local_items: items,
                    local_health,
                    used_legacy_fallback: false,
                    auto_activated_id: act_id,
                    auto_activated_label: act_label,
                    auto_activated_model: act_model,
                    warning: None,
                });
            }
        }
    }

    // Legacy v1.3.0 server fallback: inspect current state and, if no real provider is active yet,
    // refresh the best local subscription's OAuth token locally and push via `/magpie/providers`.
    check_pinned_epoch(pinned, token)?;
    let current_state = client.hermes.magpie_state(token).await.ok();
    let needs_bootstrap = current_state.as_ref().is_none_or(|st| {
        st.needs_provider_selection
            || st.active_provider.trim().is_empty()
            || st.active_provider == "local_hermes"
    });

    if auto_activate && needs_bootstrap {
        let candidates = pick_ordered_bootstrap_candidates(&items);
        let mut last_err: Option<String> = None;
        for cand in candidates {
            if let Some(h) = local_health.get(&cand.account.id).filter(|h| !h.usable) {
                last_err = Some(format!(
                    "{}: {}",
                    cand.account.label,
                    h.reason.as_deref().unwrap_or("凭据不可用")
                ));
                continue;
            }
            check_pinned_epoch(pinned, token)?;
            match resolve_fresh_bearer_token(&http, &cand).await {
                Ok(bearer) => {
                    check_pinned_epoch(pinned, token)?;
                    let req = build_legacy_save_provider_request(&cand, bearer, None, true);
                    let chosen_model = req.default_model.clone();
                    match client.hermes.magpie_save_provider(token, &req).await {
                        Ok(saved) => {
                            return Ok(LocalSubscriptionSyncOutcome {
                                state: Some(saved.state),
                                local_items: items,
                                local_health,
                                used_legacy_fallback: true,
                                auto_activated_id: Some(cand.account.id.clone()),
                                auto_activated_label: Some(cand.account.label.clone()),
                                auto_activated_model: chosen_model,
                                warning: None,
                            });
                        }
                        Err(e) => {
                            last_err = Some(format!("save provider '{}': {e}", cand.account.id));
                        }
                    }
                }
                Err(e) => {
                    let code = classify_provider_auth_error_code(&e);
                    let formatted = format_provider_unavailable_reason(&cand.account.id, &e);
                    local_health.insert(
                        cand.account.id.clone(),
                        LocalSubscriptionHealth::unavailable(code, formatted),
                    );
                    last_err = Some(format!("{}: {e}", cand.account.label));
                }
            }
        }
        return Ok(LocalSubscriptionSyncOutcome {
            state: current_state,
            local_items: items,
            local_health,
            used_legacy_fallback: true,
            auto_activated_id: None,
            auto_activated_label: None,
            auto_activated_model: None,
            warning: last_err,
        });
    }

    Ok(LocalSubscriptionSyncOutcome {
        state: current_state,
        local_items: items,
        local_health,
        used_legacy_fallback: true,
        auto_activated_id: None,
        auto_activated_label: None,
        auto_activated_model: None,
        warning: None,
    })
}

fn url_encode_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Outcome of activating a subscription card (native or legacy fallback).
pub struct SubscriptionActivationOutcome {
    pub active_provider_id: String,
    pub active_model: String,
    pub state: Option<MagpieStateDto>,
    pub used_legacy_fallback: bool,
}

/// Activates a subscription by ID, trying the native `/subscriptions/{id}/activate` endpoint first
/// and seamlessly falling back to local OAuth token refresh + `/magpie/providers` upsert when
/// talking to a legacy v1.3.0 server that doesn't store client-discovered subscriptions.
pub async fn activate_subscription_with_local_fallback(
    client: &Arc<TelegramClient>,
    base_url: &str,
    token: &str,
    sub_id: &str,
    model: Option<&str>,
    local_items: &[ClientSubscriptionSyncItem],
) -> Result<SubscriptionActivationOutcome, String> {
    activate_subscription_with_local_fallback_inner(
        client,
        None,
        base_url,
        token,
        sub_id,
        model,
        local_items,
    )
    .await
}

/// Session-epoch-pinned variant of [`activate_subscription_with_local_fallback`].
pub async fn activate_subscription_with_local_fallback_pinned(
    client: &Arc<TelegramClient>,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
    sub_id: &str,
    model: Option<&str>,
    local_items: &[ClientSubscriptionSyncItem],
) -> Result<SubscriptionActivationOutcome, String> {
    activate_subscription_with_local_fallback_inner(
        client,
        Some(expected_epoch),
        base_url,
        token,
        sub_id,
        model,
        local_items,
    )
    .await
}

async fn activate_subscription_with_local_fallback_inner(
    client: &Arc<TelegramClient>,
    pinned_epoch: Option<u64>,
    base_url: &str,
    token: &str,
    sub_id: &str,
    model: Option<&str>,
    local_items: &[ClientSubscriptionSyncItem],
) -> Result<SubscriptionActivationOutcome, String> {
    let pinned = pinned_epoch.map(|e| (client.as_ref(), e));
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(20)),
    );
    let act_url = format!(
        "{}/api/v1/hermes/magpie/subscriptions/{}/activate",
        base_url.trim_end_matches('/'),
        url_encode_segment(sub_id)
    );
    let payload = SubscriptionActivateRequest {
        model: model.map(str::to_string),
    };

    check_pinned_epoch(pinned, token)?;
    if let Ok(resp) = http
        .post(&act_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&payload)
        .send()
        .await
    {
        let status = resp.status();
        if status.is_success() {
            if let Ok(parsed) = resp.json::<SubscriptionActivateResponse>().await {
                check_pinned_epoch(pinned, token)?;
                let state = client.hermes.magpie_state(token).await.ok();
                return Ok(SubscriptionActivationOutcome {
                    active_provider_id: parsed.active_provider_id,
                    active_model: parsed.active_model,
                    state,
                    used_legacy_fallback: false,
                });
            }
        } else if status != reqwest::StatusCode::NOT_FOUND
            && status != reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            // Explicit server rejection (e.g. 400 `provider_unavailable` after OAuth verification):
            // NEVER bypass with legacy fallback.
            let body = resp.text().await.unwrap_or_default();
            let parsed_msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .or_else(|| v.get("error"))
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                });
            let err_msg = parsed_msg
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("Server rejected activation ({status})"));
            return Err(err_msg);
        }
    }

    // Legacy fallback: find the local item (or rescan), refresh OAuth token locally, and save provider
    check_pinned_epoch(pinned, token)?;
    let matched = local_items
        .iter()
        .find(|i| i.account.id == sub_id)
        .cloned()
        .or_else(|| {
            discover_local_subscriptions_for_server(base_url)
                .into_iter()
                .find(|i| i.account.id == sub_id)
        })
        .ok_or_else(|| format!("Subscription '{sub_id}' not found on server or local machine"))?;

    check_pinned_epoch(pinned, token)?;
    let bearer = resolve_fresh_bearer_token(&http, &matched).await?;
    check_pinned_epoch(pinned, token)?;
    let req = build_legacy_save_provider_request(&matched, bearer, model, true);
    let chosen_model = req.default_model.clone().unwrap_or_default();
    let saved = client
        .hermes
        .magpie_save_provider(token, &req)
        .await
        .map_err(|e| format!("Failed to sync & activate provider '{sub_id}': {e}"))?;

    Ok(SubscriptionActivationOutcome {
        active_provider_id: matched.account.id,
        active_model: if saved.state.active_model.is_empty() {
            chosen_model
        } else {
            saved.state.active_model.clone()
        },
        state: Some(saved.state),
        used_legacy_fallback: true,
    })
}

/// Returns the local disk cache path for the compact `models.dev` live catalog.
pub fn local_models_dev_cache_path() -> Option<PathBuf> {
    resolve_local_home_dir().map(|h| {
        h.join(".config")
            .join("gpui_telegram")
            .join("models_dev_catalog_cache.json")
    })
}

/// Fetches the live `models.dev` catalog: prefers the fast server-cached endpoint
/// `GET /api/v1/hermes/magpie/models/catalog` (~15 KB) and falls back to direct download + local
/// disk cache when connected to a legacy server or offline.
pub async fn fetch_live_models_catalog(
    base_url: &str,
    token: &str,
    force_refresh: bool,
) -> Result<CompactModelsCatalog, String> {
    fetch_live_models_catalog_inner(None, base_url, token, force_refresh).await
}

/// Session-epoch-pinned variant of [`fetch_live_models_catalog`].
pub async fn fetch_live_models_catalog_pinned(
    client: &TelegramClient,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
    force_refresh: bool,
) -> Result<CompactModelsCatalog, String> {
    fetch_live_models_catalog_inner(Some((client, expected_epoch)), base_url, token, force_refresh)
        .await
}

async fn fetch_live_models_catalog_inner(
    pinned: Option<(&TelegramClient, u64)>,
    base_url: &str,
    token: &str,
    force_refresh: bool,
) -> Result<CompactModelsCatalog, String> {
    check_pinned_epoch(pinned, token)?;
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(20)),
    );
    let cache_path = local_models_dev_cache_path();

    // 1. Try fast server-side catalog proxy (~15 KB, <100ms on cloud VM)
    let server_url = format!(
        "{}/api/v1/hermes/magpie/models/catalog{}",
        base_url.trim_end_matches('/'),
        if force_refresh { "?refresh=true" } else { "" }
    );
    if let Ok(Ok(resp)) = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        http.get(&server_url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .send(),
    )
    .await
    {
        if resp.status().is_success() {
            if let Ok(catalog) = resp.json::<CompactModelsCatalog>().await {
                if !catalog.providers.is_empty() {
                    if let Some(ref p) = cache_path {
                        if let Some(parent) = p.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Ok(raw) = serde_json::to_string(&catalog) {
                            let _ = std::fs::write(p, raw);
                        }
                    }
                    return Ok(catalog);
                }
            }
        }
    }

    // 2. Fallback: direct models.dev fetch with local 6h disk cache
    if std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
        return Err("models.dev catalog fallback skipped in GPUI_OFFLINE_TEST".to_string());
    }
    check_pinned_epoch(pinned, token)?;
    fetch_models_dev_catalog(&http, cache_path.as_deref(), force_refresh).await
}

/// Enriches discovered subscription items with live models from `CompactModelsCatalog`
/// (`models.dev`), placing live catalog models ahead of any fallback entries.
pub fn enrich_items_with_live_catalog(
    items: &mut [ClientSubscriptionSyncItem],
    catalog: &CompactModelsCatalog,
) {
    for item in items.iter_mut() {
        let live_models = catalog.models_for_subscription(item.account.kind);
        if !live_models.is_empty() {
            // Keep any real local CLI cache models (e.g. ~/.codex/models_cache.json) + live models.dev
            let merged = merge_discovered_models(&[&live_models, &item.available_models]);
            item.available_models = merged;
        }
    }
}

pub struct LiveModelsFetchOutcome {
    pub models: Vec<String>,
    pub source_label_zh: String,
    pub source_label_en: String,
}

/// Fetches the latest live model list for a single discovered subscription card by querying:
/// 1. The upstream provider's real `/models` API (Vertex AI / Codex / Gemini / Anthropic / OpenAI)
/// 2. The live `models.dev` catalog (via server proxy or direct cached fetch)
pub async fn fetch_models_for_subscription_live(
    base_url: &str,
    token: &str,
    item: &ClientSubscriptionSyncItem,
) -> Result<LiveModelsFetchOutcome, String> {
    fetch_models_for_subscription_live_inner(None, base_url, token, item).await
}

/// Session-epoch-pinned variant of [`fetch_models_for_subscription_live`].
pub async fn fetch_models_for_subscription_live_pinned(
    client: &TelegramClient,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
    item: &ClientSubscriptionSyncItem,
) -> Result<LiveModelsFetchOutcome, String> {
    fetch_models_for_subscription_live_inner(
        Some((client, expected_epoch)),
        base_url,
        token,
        item,
    )
    .await
}

async fn fetch_models_for_subscription_live_inner(
    pinned: Option<(&TelegramClient, u64)>,
    base_url: &str,
    token: &str,
    item: &ClientSubscriptionSyncItem,
) -> Result<LiveModelsFetchOutcome, String> {
    check_pinned_epoch(pinned, token)?;
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(20)),
    );
    let bearer_opt = resolve_fresh_bearer_token(&http, item).await.ok();

    check_pinned_epoch(pinned, token)?;
    let upstream_models = fetch_live_upstream_models(
        &http,
        item.account.kind,
        item.account.base_url.as_deref(),
        bearer_opt.as_deref(),
        &item.raw_headers,
        item.account.account_id.as_deref(),
        item.region_id.as_deref(),
    )
    .await
    .unwrap_or_default();

    let catalog_models = fetch_live_models_catalog_inner(pinned, base_url, token, true)
        .await
        .map(|cat| cat.models_for_subscription(item.account.kind))
        .unwrap_or_default();

    check_pinned_epoch(pinned, token)?;
    let merged = merge_discovered_models(&[&upstream_models, &catalog_models, &item.available_models]);
    if merged.is_empty() {
        return Err("未能从上游接口或 models.dev 获取到可用模型".to_string());
    }

    let (source_label_zh, source_label_en) = match (!upstream_models.is_empty(), !catalog_models.is_empty()) {
        (true, true) => ("上游供应商接口 + models.dev".to_string(), "Upstream API + models.dev".to_string()),
        (true, false) => ("上游供应商接口".to_string(), "Upstream API".to_string()),
        (false, true) => ("models.dev 实时目录".to_string(), "models.dev live catalog".to_string()),
        (false, false) => ("本机缓存".to_string(), "local cache".to_string()),
    };

    Ok(LiveModelsFetchOutcome {
        models: merged,
        source_label_zh,
        source_label_en,
    })
}

/// Refreshes the bearer token for an active legacy-synced subscription and updates it silently on
/// the server (`activate: false`) so long desktop sessions never expire.
pub async fn refresh_legacy_provider_token_once(
    client: &Arc<TelegramClient>,
    base_url: &str,
    token: &str,
    sub_id: &str,
    model_override: Option<&str>,
) -> Result<(), String> {
    refresh_legacy_provider_token_once_inner(client, None, base_url, token, sub_id, model_override)
        .await
}

/// Session-epoch-pinned variant of [`refresh_legacy_provider_token_once`].
pub async fn refresh_legacy_provider_token_once_pinned(
    client: &Arc<TelegramClient>,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
    sub_id: &str,
    model_override: Option<&str>,
) -> Result<(), String> {
    refresh_legacy_provider_token_once_inner(
        client,
        Some(expected_epoch),
        base_url,
        token,
        sub_id,
        model_override,
    )
    .await
}

async fn refresh_legacy_provider_token_once_inner(
    client: &Arc<TelegramClient>,
    pinned_epoch: Option<u64>,
    base_url: &str,
    token: &str,
    sub_id: &str,
    model_override: Option<&str>,
) -> Result<(), String> {
    let pinned = pinned_epoch.map(|e| (client.as_ref(), e));
    check_pinned_epoch(pinned, token)?;
    let items = discover_local_subscriptions_for_server(base_url);
    check_pinned_epoch(pinned, token)?;
    let matched = items
        .into_iter()
        .find(|i| i.account.id == sub_id)
        .ok_or_else(|| format!("Local subscription '{sub_id}' no longer present"))?;
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(Duration::from_secs(10)),
        Some(Duration::from_secs(20)),
    );
    let bearer = resolve_fresh_bearer_token(&http, &matched).await?;
    check_pinned_epoch(pinned, token)?;
    let req = build_legacy_save_provider_request(&matched, bearer, model_override, false);
    client
        .hermes
        .magpie_save_provider(token, &req)
        .await
        .map(|_| ())
        .map_err(|e| format!("Legacy token keep-alive update failed: {e}"))
}

/// Computes a deterministic SHA-256 fingerprint over discovered local AI subscription items.
pub fn discovered_subscriptions_fingerprint(items: &[ClientSubscriptionSyncItem]) -> String {
    let bytes = serde_json::to_vec(items).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!("{digest:x}")
}

/// Builds a client-side E2EE `AiOauthVaultBundle` containing cross-device-safe AI subscriptions
/// and a sanitized `ProviderRouterState` (stripping loopback proxies and rotating/ephemeral OAuth).
pub fn build_ai_oauth_vault_bundle(
    items: &[ClientSubscriptionSyncItem],
    router: Option<&ProviderRouter>,
) -> Result<AiOauthVaultBundle, String> {
    let syncable_items: Vec<&ClientSubscriptionSyncItem> = items
        .iter()
        .filter(|i| is_cross_device_syncable_subscription(i))
        .collect();
    let subscriptions_json = serde_json::to_value(&syncable_items)
        .map_err(|e| format!("Failed serializing discovered subscriptions: {e}"))?;
    let providers_json = match router {
        Some(r) => {
            let clean = sanitize_router_state_for_cross_device(&r.snapshot());
            serde_json::to_value(clean)
                .map_err(|e| format!("Failed serializing provider router snapshot: {e}"))?
        }
        None => serde_json::Value::Null,
    };
    Ok(AiOauthVaultBundle {
        providers_json,
        subscriptions_json,
        updated_at_ms: Utc::now().timestamp_millis(),
    })
}

/// Returns `true` when `router` has a usable non-placeholder AI provider or active subscription.
pub fn is_provider_router_usable(router: &ProviderRouter) -> bool {
    if router.needs_provider_selection() {
        return false;
    }
    let (active_id, active_model, active_mode) = router.active_info();
    let active_ok = active_mode != ApiMode::LocalHermes
        && active_id != "local_hermes"
        && !active_model.eq_ignore_ascii_case("hermes-local-v1")
        && router.provider_availability(&active_id).usable;
    active_ok || router.has_configured_provider() || router.has_active_subscription()
}

/// Outcome of a one-click refresh & re-activate attempt for a provider or subscription.
pub struct ProviderRefreshOutcome {
    pub provider_id: String,
    pub usable: bool,
    pub activated: bool,
    pub active_model: Option<String>,
    pub unavailable_code: Option<String>,
    pub unavailable_reason: Option<String>,
    pub state: Option<MagpieStateDto>,
    pub local_items: Vec<ClientSubscriptionSyncItem>,
    pub local_health: HashMap<String, LocalSubscriptionHealth>,
    pub used_legacy_fallback: bool,
}

/// Discovers all local subscriptions under `$HOME` regardless of whether the remote cloud server
/// accepts cross-device sync (used for in-process `FabushiBotHost` refresh after local re-login).
pub fn discover_all_local_subscriptions_in_home() -> Vec<ClientSubscriptionSyncItem> {
    if std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
        return Vec::new();
    }
    let Some(home) = resolve_local_home_dir() else {
        return Vec::new();
    };
    let codex_override = std::env::var_os("CODEX_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    discover_syncable_subscriptions_in_home(&home, codex_override.as_deref(), false)
}

fn apply_unavailable_override_to_magpie_state(
    state: &mut Option<MagpieStateDto>,
    target_id: &str,
    code: Option<&str>,
    reason: Option<&str>,
) {
    let Some(st) = state.as_mut() else {
        return;
    };
    let clean_code = code
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let clean_reason = reason
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    for s in &mut st.subscriptions {
        if s.id == target_id {
            s.usable = false;
            s.active = false;
            if clean_code.is_some() {
                s.unavailable_code = clean_code.clone();
            }
            if clean_reason.is_some() {
                s.unavailable_reason = clean_reason.clone();
            }
        }
    }
    for p in &mut st.providers {
        if p.provider_id == target_id {
            p.usable = false;
            p.is_active = false;
            if clean_code.is_some() {
                p.unavailable_code = clean_code.clone();
            }
            if clean_reason.is_some() {
                p.unavailable_reason = clean_reason.clone();
            }
        }
    }
}

/// Force-refreshes local & server credentials for `target_id` (optionally matching by `kind_hint`
/// when a user re-logged into a different account email for the same subscription family),
/// verifies availability live, and atomically activates it when usable.
pub async fn refresh_and_activate_provider_or_subscription_pinned(
    client: &Arc<TelegramClient>,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
    target_id: &str,
    kind_hint: Option<SubscriptionKind>,
    model_override: Option<&str>,
) -> Result<ProviderRefreshOutcome, String> {
    magpie_gateway::invalidate_system_proxy_cache();
    client
        .ensure_pinned_session(expected_epoch, token)
        .map_err(|e| format!("Session changed before provider refresh: {e}"))?;

    let scan_base = base_url.to_string();
    let mut local_items = tokio::task::spawn_blocking(move || {
        discover_local_subscriptions_for_server(&scan_base)
    })
    .await
    .unwrap_or_default();

    if let Ok(cat) =
        fetch_live_models_catalog_pinned(client, expected_epoch, base_url, token, false).await
    {
        enrich_items_with_live_catalog(&mut local_items, &cat);
    }

    let mut local_health = verify_discovered_subscriptions(&local_items).await;

    // Resolve whether `target_id` (or `kind_hint`) maps to a freshly discovered local subscription.
    let matched_local = local_items
        .iter()
        .find(|i| {
            i.account.id == target_id
                || (i.account.kind == SubscriptionKind::GoogleVertex
                    && target_id == "magpie_provider:google-vertex")
        })
        .or_else(|| {
            kind_hint.and_then(|kind| {
                local_items
                    .iter()
                    .find(|i| {
                        i.account.kind == kind
                            && local_health.get(&i.account.id).is_none_or(|h| h.usable)
                    })
                    .or_else(|| local_items.iter().find(|i| i.account.kind == kind))
            })
        })
        .cloned();

    if let Some(local_item) = matched_local {
        let resolved_id = local_item.account.id.clone();
        let is_remote_server = !is_loopback_server_url(base_url);
        if let Some(h) = local_health.get(&resolved_id).filter(|h| !h.usable).cloned() {
            // If connected to a remote cloud server and the local machine only failed with a
            // network reachability error (e.g. Mac without proxy reaching oauth2.googleapis.com),
            // still upload fresh credentials and let the remote server perform authoritative live
            // verification & activation. Definitive auth failures (invalid_grant/401/expired)
            // and loopback servers always stay strictly blocked here.
            let allow_remote_server_verify =
                is_remote_server && h.error_code.as_deref() == Some("unreachable");
            if !allow_remote_server_verify {
                let mut sync_outcome = sync_local_subscriptions_to_server_pinned(
                    client,
                    expected_epoch,
                    base_url,
                    token,
                    local_items,
                    false,
                )
                .await?;
                apply_unavailable_override_to_magpie_state(
                    &mut sync_outcome.state,
                    &resolved_id,
                    h.error_code.as_deref(),
                    h.reason.as_deref(),
                );
                sync_outcome
                    .local_health
                    .insert(resolved_id.clone(), h.clone());
                return Ok(ProviderRefreshOutcome {
                    provider_id: resolved_id,
                    usable: false,
                    activated: false,
                    active_model: None,
                    unavailable_code: h.error_code,
                    unavailable_reason: h.reason,
                    state: sync_outcome.state,
                    local_items: sync_outcome.local_items,
                    local_health: sync_outcome.local_health,
                    used_legacy_fallback: sync_outcome.used_legacy_fallback,
                });
            }
        }

        let sync_outcome = sync_local_subscriptions_to_server_pinned(
            client,
            expected_epoch,
            base_url,
            token,
            local_items,
            false,
        )
        .await?;
        let act_res = activate_subscription_with_local_fallback_pinned(
            client,
            expected_epoch,
            base_url,
            token,
            &resolved_id,
            model_override,
            &sync_outcome.local_items,
        )
        .await;
        return match act_res {
            Ok(act) => {
                let mut health = sync_outcome.local_health;
                health.insert(resolved_id.clone(), LocalSubscriptionHealth::usable());
                Ok(ProviderRefreshOutcome {
                    provider_id: act.active_provider_id,
                    usable: true,
                    activated: true,
                    active_model: Some(act.active_model),
                    unavailable_code: None,
                    unavailable_reason: None,
                    state: act.state.or(sync_outcome.state),
                    local_items: sync_outcome.local_items,
                    local_health: health,
                    used_legacy_fallback: act.used_legacy_fallback,
                })
            }
            Err(err) => {
                let code = classify_provider_auth_error_code(&err);
                let mut health = sync_outcome.local_health;
                health.insert(
                    resolved_id.clone(),
                    LocalSubscriptionHealth::unavailable(code, err.clone()),
                );
                let mut refreshed_state = client
                    .hermes_magpie_state_with_token(token, expected_epoch)
                    .await
                    .ok()
                    .or(sync_outcome.state);
                apply_unavailable_override_to_magpie_state(
                    &mut refreshed_state,
                    &resolved_id,
                    Some(code),
                    Some(&err),
                );
                Ok(ProviderRefreshOutcome {
                    provider_id: resolved_id,
                    usable: false,
                    activated: false,
                    active_model: None,
                    unavailable_code: Some(code.to_string()),
                    unavailable_reason: Some(err),
                    state: refreshed_state,
                    local_items: sync_outcome.local_items,
                    local_health: health,
                    used_legacy_fallback: sync_outcome.used_legacy_fallback,
                })
            }
        };
    }

    // Server-side API-key / custom / local / server-managed subscription refresh endpoint.
    let refresh_req = MagpieRefreshProviderRequest {
        activate: true,
        model: model_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    };
    match client
        .hermes_magpie_refresh_provider_with_token(
            token,
            expected_epoch,
            target_id,
            Some(&refresh_req),
        )
        .await
    {
        Ok(resp) => {
            if resp.usable {
                local_health.insert(resp.provider_id.clone(), LocalSubscriptionHealth::usable());
            } else {
                local_health.insert(
                    resp.provider_id.clone(),
                    LocalSubscriptionHealth::unavailable(
                        resp.unavailable_code.clone().unwrap_or_default(),
                        resp.unavailable_reason.clone().unwrap_or_default(),
                    ),
                );
            }
            Ok(ProviderRefreshOutcome {
                provider_id: resp.provider_id,
                usable: resp.usable,
                activated: resp.activated,
                active_model: (!resp.active_model.trim().is_empty()).then_some(resp.active_model),
                unavailable_code: resp.unavailable_code,
                unavailable_reason: resp.unavailable_reason,
                state: Some(resp.state),
                local_items,
                local_health,
                used_legacy_fallback: false,
            })
        }
        Err(err) => {
            // Legacy server fallback when `/providers/{id}/refresh` returns 404/405:
            // probe first, and only activate when probe succeeds.
            let err_str = err.to_string();
            if err_str.contains("404") || err_str.contains("405") {
                let probe_req = MagpieProbeRequest {
                    model: model_override.map(str::to_string),
                    prompt: Some("Reply with OK".to_string()),
                };
                let probe = client
                    .hermes_magpie_probe_provider_with_token(
                        token,
                        expected_epoch,
                        target_id,
                        Some(&probe_req),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                if probe.ok {
                    let save_req = MagpieSaveProviderRequest {
                        provider_id: Some(target_id.to_string()),
                        default_model: model_override.map(str::to_string),
                        enabled: Some(true),
                        activate: true,
                        ..Default::default()
                    };
                    let saved = client
                        .hermes_magpie_save_provider_with_token(token, expected_epoch, &save_req)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(ProviderRefreshOutcome {
                        provider_id: target_id.to_string(),
                        usable: true,
                        activated: true,
                        active_model: Some(saved.state.active_model.clone()),
                        unavailable_code: None,
                        unavailable_reason: None,
                        state: Some(saved.state),
                        local_items,
                        local_health,
                        used_legacy_fallback: true,
                    });
                }
                let reason = probe
                    .error
                    .unwrap_or_else(|| "连通性检测失败，服务商当前不可用".to_string());
                let code = classify_provider_auth_error_code(&reason);
                let state = client
                    .hermes_magpie_state_with_token(token, expected_epoch)
                    .await
                    .ok();
                return Ok(ProviderRefreshOutcome {
                    provider_id: target_id.to_string(),
                    usable: false,
                    activated: false,
                    active_model: None,
                    unavailable_code: Some(code.to_string()),
                    unavailable_reason: Some(reason),
                    state,
                    local_items,
                    local_health,
                    used_legacy_fallback: true,
                });
            }
            Err(err_str)
        }
    }
}

/// Applies locally verified subscription availability onto the in-process `FabushiBotHost`
/// so dead/revoked local credentials (e.g. Google Vertex AI `invalid_grant`) are immediately
/// marked unavailable and demoted from `active_provider_id` in local Fabushi execution too.
pub fn apply_local_subscription_health_to_fabushi_for_user(
    host: &FabushiBotHost,
    health: &HashMap<String, LocalSubscriptionHealth>,
    user_id: Option<&str>,
) {
    if health.is_empty() {
        return;
    }
    let core = host.core_engine();
    let router = if let Some(uid) = user_id.map(str::trim).filter(|s| !s.is_empty()) {
        core.providers_for_user(uid)
    } else {
        core.providers().clone()
    };
    let mut changed = false;
    for (id, status) in health {
        if status.usable {
            router.mark_provider_usable(id);
            changed = true;
        } else {
            let code = status
                .error_code
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .unwrap_or("local_verify_failed");
            let msg = status
                .reason
                .clone()
                .unwrap_or_else(|| "凭据验证失败，当前不可用".to_string());
            router.mark_provider_unavailable(id, code, msg);
            changed = true;
        }
    }
    if router.reconcile_active_provider() || changed {
        if let Some(uid) = user_id.map(str::trim).filter(|s| !s.is_empty()) {
            core.persist_user_providers_state(uid);
        }
        host.sync_all_providers();
    }
}

/// Hydrates locally discovered AI OAuth / ADC subscriptions directly into the desktop's
/// in-process `FabushiBotHost` for `user_id` (or global base router when `user_id` is `None`).
pub fn hydrate_local_subscriptions_into_fabushi_for_user(
    host: &FabushiBotHost,
    items: &[ClientSubscriptionSyncItem],
    user_id: Option<&str>,
) -> usize {
    if items.is_empty() {
        return 0;
    }
    let count = items.len();
    let core = host.core_engine();
    if let Some(uid) = user_id.map(str::trim).filter(|s| !s.is_empty()) {
        core.providers_for_user(uid)
            .ingest_client_subscriptions(items.to_vec(), true);
        core.persist_user_providers_state(uid);
    } else {
        core.providers().ingest_client_subscriptions(items.to_vec(), true);
    }
    host.sync_all_providers();
    count
}

/// Hydrates locally discovered AI OAuth / ADC subscriptions directly into the desktop's
/// in-process `FabushiBotHost` (`core_engine` + all provisioned bot instances).
pub fn hydrate_local_subscriptions_into_fabushi(
    host: &FabushiBotHost,
    items: &[ClientSubscriptionSyncItem],
) -> usize {
    hydrate_local_subscriptions_into_fabushi_for_user(host, items, None)
}

/// Hydrates a decrypted `AiOauthVaultBundle` into the desktop's in-process `FabushiBotHost`
/// for `user_id` (or global base router when `user_id` is `None`), stripping any legacy
/// loopback proxies or rotating/ephemeral OAuth entries pulled from older vault snapshots.
pub fn hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
    host: &FabushiBotHost,
    bundle: &AiOauthVaultBundle,
    user_id: Option<&str>,
) -> Result<usize, String> {
    let mut count = 0usize;
    let core = host.core_engine();
    let normalized_uid = user_id.map(str::trim).filter(|s| !s.is_empty());
    if !bundle.providers_json.is_null() {
        if let Ok(raw_router_state) =
            serde_json::from_value::<ProviderRouterState>(bundle.providers_json.clone())
        {
            let router_state = sanitize_router_state_for_cross_device(&raw_router_state);
            let usable_provider_count = router_state
                .providers
                .values()
                .filter(|p| {
                    p.enabled
                        && p.id != "local_hermes"
                        && p.api_mode != ApiMode::LocalHermes
                        && (!p.credentials.is_empty() || p.no_key_required)
                })
                .count();
            let usable_sub_count = router_state
                .subscriptions
                .iter()
                .filter(|s| {
                    s.active
                        && router_state.providers.get(&s.id).is_none_or(|p| p.enabled)
                        && (s.access_token
                            .as_deref()
                            .is_some_and(|t| !t.trim().is_empty())
                            || router_state.synced_vertex_adcs.contains_key(&s.id)
                            || router_state.providers.get(&s.id).is_some_and(|p| {
                                p.enabled && (!p.credentials.is_empty() || p.no_key_required)
                            }))
                })
                .count();
            let has_non_placeholder = usable_provider_count > 0 || usable_sub_count > 0;
            if has_non_placeholder {
                if let Some(uid) = normalized_uid {
                    core.providers_for_user(uid).sync_from_state(&router_state);
                    core.persist_user_providers_state(uid);
                } else {
                    core.providers().sync_from_state(&router_state);
                }
                count = count.max(usable_provider_count.max(usable_sub_count).max(1));
            }
        }
    }
    if let Ok(raw_items) =
        serde_json::from_value::<Vec<ClientSubscriptionSyncItem>>(bundle.subscriptions_json.clone())
    {
        let items: Vec<ClientSubscriptionSyncItem> = raw_items
            .into_iter()
            .filter(is_cross_device_syncable_subscription)
            .collect();
        if !items.is_empty() {
            let sub_count =
                hydrate_local_subscriptions_into_fabushi_for_user(host, &items, normalized_uid);
            return Ok(count.max(sub_count));
        }
    }
    host.sync_all_providers();
    Ok(count)
}

/// Hydrates a decrypted `AiOauthVaultBundle` into the desktop's in-process `FabushiBotHost`.
pub fn hydrate_ai_oauth_vault_bundle_into_fabushi(
    host: &FabushiBotHost,
    bundle: &AiOauthVaultBundle,
) -> Result<usize, String> {
    hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(host, bundle, None)
}

/// Returns `true` when `host` has a usable non-placeholder AI provider configured locally for `user_id`.
pub fn has_usable_local_fabushi_provider_for_user(
    host: &FabushiBotHost,
    user_id: Option<&str>,
) -> bool {
    let router = host.core_engine().resolve_providers(user_id);
    is_provider_router_usable(&router)
}

/// Returns `true` when `host` has a usable non-placeholder AI provider configured locally.
pub fn has_usable_local_fabushi_provider(host: &FabushiBotHost) -> bool {
    has_usable_local_fabushi_provider_for_user(host, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use magpie_gateway::{ProtocolKind, SubscriptionAccount, VertexAdcCredentials};
    use std::collections::BTreeMap;

    fn sample_vertex_item() -> ClientSubscriptionSyncItem {
        let mut headers = BTreeMap::new();
        headers.insert("x-goog-user-project".to_string(), "proj-123".to_string());
        ClientSubscriptionSyncItem {
            account: SubscriptionAccount {
                id: "google_vertex:dev@example.com".to_string(),
                kind: SubscriptionKind::GoogleVertex,
                label: "Google Vertex AI (dev@example.com · proj-123)".to_string(),
                email: Some("dev@example.com".to_string()),
                account_id: Some("proj-123".to_string()),
                plan: Some("vertex_adc".to_string()),
                active: true,
                base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
                protocol: ProtocolKind::GeminiGenerateContent,
                default_model: Some("gemini-2.5-flash".to_string()),
                source_path: "/Users/test/.config/gcloud/application_default_credentials.json"
                    .to_string(),
                expires_at_ms: None,
                access_token: None,
                headers: headers.clone(),
            },
            access_token: None,
            raw_headers: headers,
            vertex_adc: Some(VertexAdcCredentials {
                client_id: "cid".to_string(),
                client_secret: "csec".to_string(),
                refresh_token: "rt".to_string(),
                quota_project_id: Some("proj-123".to_string()),
                account: Some("dev@example.com".to_string()),
            }),
            codex_oauth: None,
            region_id: Some("global".to_string()),
            available_models: Vec::new(),
        }
    }

    fn sample_codex_item() -> ClientSubscriptionSyncItem {
        let mut headers = BTreeMap::new();
        headers.insert("originator".to_string(), "magpie".to_string());
        headers.insert("ChatGPT-Account-ID".to_string(), "acct_team_1".to_string());
        ClientSubscriptionSyncItem {
            account: SubscriptionAccount {
                id: "magpie_codex:abc123".to_string(),
                kind: SubscriptionKind::CodexChatGpt,
                label: "Codex (dev@example.com · Team)".to_string(),
                email: Some("dev@example.com".to_string()),
                account_id: Some("acct_team_1".to_string()),
                plan: Some("team".to_string()),
                active: true,
                base_url: Some("https://chatgpt.com/backend-api/codex".to_string()),
                protocol: ProtocolKind::CodexResponses,
                default_model: Some("codex-mini-latest".to_string()),
                source_path: "/Users/test/.config/magpie/logins.json".to_string(),
                expires_at_ms: None,
                access_token: Some("eyJ-codex-token".to_string()),
                headers: headers.clone(),
            },
            access_token: Some("eyJ-codex-token".to_string()),
            raw_headers: headers,
            vertex_adc: None,
            codex_oauth: None,
            region_id: None,
            available_models: Vec::new(),
        }
    }

    #[test]
    fn bootstrap_candidates_prioritize_vertex_adc_then_codex() {
        let codex = sample_codex_item();
        let vertex = sample_vertex_item();
        let ordered = pick_ordered_bootstrap_candidates(&[codex.clone(), vertex.clone()]);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].account.id, vertex.account.id);
        assert_eq!(ordered[1].account.id, codex.account.id);
    }

    #[test]
    fn legacy_save_request_maps_vertex_and_codex_cleanly() {
        let v_req = build_legacy_save_provider_request(
            &sample_vertex_item(),
            "ya29.fresh-token".to_string(),
            Some("gemini-2.5-pro"),
            true,
        );
        assert_eq!(v_req.provider_id.as_deref(), Some("google_vertex:dev@example.com"));
        assert_eq!(v_req.api_mode.as_deref(), Some("google_vertex"));
        assert_eq!(v_req.default_model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(v_req.region_id.as_deref(), Some("global"));
        assert_eq!(v_req.api_key.as_deref(), Some("ya29.fresh-token"));
        assert_eq!(
            v_req.custom_headers.get("x-goog-user-project").map(String::as_str),
            Some("proj-123")
        );
        assert!(v_req.activate);

        let c_req = build_legacy_save_provider_request(
            &sample_codex_item(),
            "eyJ-codex-token".to_string(),
            None,
            true,
        );
        assert_eq!(c_req.provider_id.as_deref(), Some("magpie_codex:abc123"));
        assert_eq!(c_req.api_mode.as_deref(), Some("codex_responses"));
        assert_eq!(
            c_req.base_url.as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );
        assert_eq!(
            c_req.custom_headers.get("ChatGPT-Account-ID").map(String::as_str),
            Some("acct_team_1")
        );
    }

    #[test]
    fn verify_discovered_subscriptions_catches_vertex_invalid_grant_locally() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let body = r#"{"error":"invalid_grant","error_description":"Bad Request"}"#;
                    let resp = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            });

            let mut vertex = sample_vertex_item();
            vertex.raw_headers.insert(
                "x-vertex-token-url".to_string(),
                format!("http://{addr}/token"),
            );
            let health = verify_discovered_subscriptions(&[vertex.clone()]).await;
            let status = health
                .get(&vertex.account.id)
                .expect("vertex health entry present");
            assert!(
                !status.usable,
                "revoked Vertex OAuth refresh token must be marked unusable"
            );
            assert_eq!(status.error_code.as_deref(), Some("oauth_invalid_grant"));
            let r = status.reason.as_deref().unwrap_or("");
            assert!(
                r.contains("invalid_grant"),
                "expected invalid_grant in reason, got: {r}"
            );

            let dto = sync_item_to_account_dto_with_health(
                &vertex,
                Some(&vertex.account.id),
                health.get(&vertex.account.id),
            );
            assert!(!dto.usable);
            assert!(!dto.active, "unusable subscription must never be marked active");
            assert_eq!(dto.unavailable_code.as_deref(), Some("oauth_invalid_grant"));
            assert!(dto.unavailable_reason.as_deref().is_some_and(|s| s.contains("invalid_grant")));
        });
    }

    #[test]
    fn apply_unavailable_override_prevents_contradictory_active_card_state() {
        use gpui_telegram_client::net::hermes_client::{MagpieProviderDto, MagpieSubscriptionDto};

        let mut state = Some(MagpieStateDto {
            active_provider: "google_vertex:user@gmail.com".to_string(),
            active_model: "gemini-2.5-flash".to_string(),
            subscriptions: vec![MagpieSubscriptionDto {
                id: "google_vertex:user@gmail.com".to_string(),
                kind: "google_vertex".to_string(),
                label: "Google Vertex AI".to_string(),
                active: true,
                usable: true,
                ..Default::default()
            }],
            providers: vec![MagpieProviderDto {
                provider_id: "google_vertex:user@gmail.com".to_string(),
                name: "Google Vertex AI".to_string(),
                is_active: true,
                usable: true,
                ..Default::default()
            }],
            ..Default::default()
        });

        apply_unavailable_override_to_magpie_state(
            &mut state,
            "google_vertex:user@gmail.com",
            Some("unreachable"),
            Some("无法连接 Google 认证服务器"),
        );

        let st = state.expect("state preserved");
        assert!(!st.subscriptions[0].usable);
        assert!(!st.subscriptions[0].active);
        assert_eq!(
            st.subscriptions[0].unavailable_code.as_deref(),
            Some("unreachable")
        );
        assert_eq!(
            st.subscriptions[0].unavailable_reason.as_deref(),
            Some("无法连接 Google 认证服务器")
        );
        assert!(!st.providers[0].usable);
        assert!(!st.providers[0].is_active);
    }
}
