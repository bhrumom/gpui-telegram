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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use gpui_telegram_client::net::hermes_client::{MagpieSaveProviderRequest, MagpieStateDto};
use gpui_telegram_client::TelegramClient;
use magpie_gateway::{
    default_models_for_subscription_kind, discover_syncable_subscriptions_in_home,
    extract_jwt_exp_ms, fetch_live_upstream_models, fetch_models_dev_catalog,
    is_loopback_server_url, merge_discovered_models, ClientSubscriptionSyncItem, CodexTokenManager,
    CompactModelsCatalog, SubscriptionKind, VertexTokenManager, DEFAULT_TOKEN_REFRESH_LEAD_TIME_MS,
};
use serde::{Deserialize, Serialize};

use crate::views::settings::sections::settings_ai_providers::{
    SubscriptionAccountDto, SubscriptionActivateRequest, SubscriptionActivateResponse,
};

/// Per-`AppShell` state for client-side local AI subscription discovery and legacy token keep-alive.
#[derive(Debug, Default)]
pub struct ShellMagpieLocalState {
    /// Cached client-discovered local subscription bundles (secrets redacted in `Debug`).
    pub discovered_items: Vec<ClientSubscriptionSyncItem>,
    /// Auth generation for which automatic startup bootstrap has already been dispatched.
    pub bootstrapped_auth_generation: Option<u64>,
    /// Generation counter for cancelling stale legacy keep-alive loops.
    pub legacy_keepalive_generation: u64,
    /// Currently active legacy-synced subscription ID (when connected to a v1.3.0 server).
    pub active_legacy_subscription_id: Option<String>,
    /// Currently active model override for the legacy-synced subscription.
    pub active_legacy_model: Option<String>,
}

impl ShellMagpieLocalState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cached_dtos(&self, active_provider: Option<&str>) -> Vec<SubscriptionAccountDto> {
        self.discovered_items
            .iter()
            .map(|item| sync_item_to_account_dto(item, active_provider))
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

    let active = active_provider.is_some_and(|ap| {
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
    }
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
                let mgr = VertexTokenManager::new(adc.clone());
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
    pub used_legacy_fallback: bool,
    pub auto_activated_id: Option<String>,
    pub auto_activated_label: Option<String>,
    pub auto_activated_model: Option<String>,
    pub warning: Option<String>,
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
    if items.is_empty() {
        let state = client.hermes.magpie_state(token).await.ok();
        return LocalSubscriptionSyncOutcome {
            state,
            local_items: items,
            used_legacy_fallback: false,
            auto_activated_id: None,
            auto_activated_label: None,
            auto_activated_model: None,
            warning: None,
        };
    }

    let http = reqwest::Client::new();
    let sync_url = format!(
        "{}/api/v1/hermes/magpie/subscriptions/sync",
        base_url.trim_end_matches('/')
    );
    let payload = NativeSyncPayload {
        subscriptions: &items,
        auto_activate,
    };

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
                    None => client.hermes.magpie_state(token).await.ok(),
                };
                let (act_id, act_label, act_model) = state
                    .as_ref()
                    .filter(|s| !s.active_provider.is_empty() && s.active_provider != "local_hermes")
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
                return LocalSubscriptionSyncOutcome {
                    state,
                    local_items: items,
                    used_legacy_fallback: false,
                    auto_activated_id: act_id,
                    auto_activated_label: act_label,
                    auto_activated_model: act_model,
                    warning: None,
                };
            }
        }
    }

    // Legacy v1.3.0 server fallback: inspect current state and, if no real provider is active yet,
    // refresh the best local subscription's OAuth token locally and push via `/magpie/providers`.
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
            match resolve_fresh_bearer_token(&http, &cand).await {
                Ok(bearer) => {
                    let req = build_legacy_save_provider_request(&cand, bearer, None, true);
                    let chosen_model = req.default_model.clone();
                    match client.hermes.magpie_save_provider(token, &req).await {
                        Ok(saved) => {
                            return LocalSubscriptionSyncOutcome {
                                state: Some(saved.state),
                                local_items: items,
                                used_legacy_fallback: true,
                                auto_activated_id: Some(cand.account.id.clone()),
                                auto_activated_label: Some(cand.account.label.clone()),
                                auto_activated_model: chosen_model,
                                warning: None,
                            };
                        }
                        Err(e) => {
                            last_err = Some(format!("save provider '{}': {e}", cand.account.id));
                        }
                    }
                }
                Err(e) => {
                    last_err = Some(format!("{}: {e}", cand.account.label));
                }
            }
        }
        return LocalSubscriptionSyncOutcome {
            state: current_state,
            local_items: items,
            used_legacy_fallback: true,
            auto_activated_id: None,
            auto_activated_label: None,
            auto_activated_model: None,
            warning: last_err,
        };
    }

    LocalSubscriptionSyncOutcome {
        state: current_state,
        local_items: items,
        used_legacy_fallback: true,
        auto_activated_id: None,
        auto_activated_label: None,
        auto_activated_model: None,
        warning: None,
    }
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
    let http = reqwest::Client::new();
    let act_url = format!(
        "{}/api/v1/hermes/magpie/subscriptions/{}/activate",
        base_url.trim_end_matches('/'),
        url_encode_segment(sub_id)
    );
    let payload = SubscriptionActivateRequest {
        model: model.map(str::to_string),
    };

    if let Ok(resp) = http
        .post(&act_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&payload)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(parsed) = resp.json::<SubscriptionActivateResponse>().await {
                let state = client.hermes.magpie_state(token).await.ok();
                return Ok(SubscriptionActivationOutcome {
                    active_provider_id: parsed.active_provider_id,
                    active_model: parsed.active_model,
                    state,
                    used_legacy_fallback: false,
                });
            }
        }
    }

    // Legacy fallback: find the local item (or rescan), refresh OAuth token locally, and save provider
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

    let bearer = resolve_fresh_bearer_token(&http, &matched).await?;
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
    let http = reqwest::Client::new();
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
    let http = reqwest::Client::new();
    let bearer_opt = resolve_fresh_bearer_token(&http, item).await.ok();

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

    let catalog_models = fetch_live_models_catalog(base_url, token, true)
        .await
        .map(|cat| cat.models_for_subscription(item.account.kind))
        .unwrap_or_default();

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
    let items = discover_local_subscriptions_for_server(base_url);
    let matched = items
        .into_iter()
        .find(|i| i.account.id == sub_id)
        .ok_or_else(|| format!("Local subscription '{sub_id}' no longer present"))?;
    let http = reqwest::Client::new();
    let bearer = resolve_fresh_bearer_token(&http, &matched).await?;
    let req = build_legacy_save_provider_request(&matched, bearer, model_override, false);
    client
        .hermes
        .magpie_save_provider(token, &req)
        .await
        .map(|_| ())
        .map_err(|e| format!("Legacy token keep-alive update failed: {e}"))
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
}
