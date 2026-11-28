use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gpui_telegram_client::net::hermes_client::{MagpieSaveProviderRequest, MagpieStateDto};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::{
    activate_subscription_with_local_fallback, build_legacy_save_provider_request,
    pick_ordered_bootstrap_candidates, sync_item_to_account_dto, sync_local_subscriptions_to_server,
};
use gpui_telegram_desktop::views::settings::sections::settings_ai_providers::{
    SettingsAiProvidersSections, SubscriptionAccountDto,
};
use magpie_gateway::{
    discover_syncable_subscriptions_in_home, ClientSubscriptionSyncItem, ProtocolKind,
    SubscriptionAccount, SubscriptionKind, VertexAdcCredentials,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn sample_vertex_item() -> ClientSubscriptionSyncItem {
    let mut headers = BTreeMap::new();
    headers.insert("x-goog-user-project".to_string(), "proj-demo-123".to_string());
    ClientSubscriptionSyncItem {
        account: SubscriptionAccount {
            id: "google_vertex:dev@vertex.test".to_string(),
            kind: SubscriptionKind::GoogleVertex,
            label: "Google Vertex AI (dev@vertex.test · proj-demo-123)".to_string(),
            email: Some("dev@vertex.test".to_string()),
            account_id: Some("proj-demo-123".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: true,
            base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
            protocol: ProtocolKind::GeminiGenerateContent,
            default_model: Some("gemini-2.5-flash".to_string()),
            source_path: "/tmp/nonexistent_adc.json".to_string(),
            expires_at_ms: None,
            access_token: None,
            headers: headers.clone(),
        },
        access_token: None,
        raw_headers: headers,
        vertex_adc: Some(VertexAdcCredentials {
            client_id: "cid-test".to_string(),
            client_secret: "csec-test".to_string(),
            refresh_token: "rt-test".to_string(),
            quota_project_id: Some("proj-demo-123".to_string()),
            account: Some("dev@vertex.test".to_string()),
        }),
        codex_oauth: None,
        region_id: Some("global".to_string()),
        available_models: Vec::new(),
    }
}

fn sample_codex_item() -> ClientSubscriptionSyncItem {
    let mut headers = BTreeMap::new();
    headers.insert("originator".to_string(), "magpie".to_string());
    headers.insert("ChatGPT-Account-ID".to_string(), "acct_team_007".to_string());
    let mock_jwt = "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6InVzZXJAdGVhbS50ZXN0IiwiZXhwIjo0MTAyNDQ0ODAwfQ.sig";
    ClientSubscriptionSyncItem {
        account: SubscriptionAccount {
            id: "magpie_codex:team007".to_string(),
            kind: SubscriptionKind::CodexChatGpt,
            label: "Codex (user@team.test · Team)".to_string(),
            email: Some("user@team.test".to_string()),
            account_id: Some("acct_team_007".to_string()),
            plan: Some("team".to_string()),
            active: true,
            base_url: Some("https://chatgpt.com/backend-api/codex".to_string()),
            protocol: ProtocolKind::CodexResponses,
            default_model: Some("codex-mini-latest".to_string()),
            source_path: "/tmp/nonexistent_logins.json".to_string(),
            expires_at_ms: Some(4_102_444_800_000),
            access_token: Some(mock_jwt.to_string()),
            headers: headers.clone(),
        },
        access_token: Some(mock_jwt.to_string()),
        raw_headers: headers,
        vertex_adc: None,
        codex_oauth: None,
        region_id: None,
        available_models: Vec::new(),
    }
}

async fn spawn_mock_http_server(
    handler: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let handler = Arc::new(handler);

    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let h = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut total = 0usize;
                let header_end;
                loop {
                    let Ok(n) = sock.read(&mut buf[total..]).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    total += n;
                    if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = pos + 4;
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let mut content_len = 0usize;
                for line in head.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse::<usize>().unwrap_or(0);
                        }
                    }
                }
                while total < header_end + content_len {
                    let Ok(n) = sock.read(&mut buf[total..]).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                let first_line = head.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let path = parts.next().unwrap_or("/");
                let body = String::from_utf8_lossy(&buf[header_end..total]).to_string();
                let (status, resp_body) = h(method, path, &body);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    _ => "Error",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });

    format!("http://{addr}")
}

#[test]
fn settings_view_retains_local_subscriptions_when_remote_server_returns_empty_list() {
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
    let mut view = SettingsAiProvidersSections::new_live(pal, "zh-CN");

    let vertex = sample_vertex_item();
    let codex = sample_codex_item();
    let local_dtos: Vec<SubscriptionAccountDto> = [vertex, codex]
        .iter()
        .map(|i| sync_item_to_account_dto(i, None))
        .collect();
    view.sync_local_subscriptions(local_dtos);

    // Remote v1.3.0 server returns empty `subscriptions`, active_provider = codex
    let state = MagpieStateDto {
        active_provider: "magpie_codex:team007".to_string(),
        active_model: "codex-mini-latest".to_string(),
        needs_provider_selection: false,
        subscriptions: Vec::new(),
        ..Default::default()
    };
    view.sync_state(state);

    let eff = view.effective_subscriptions();
    assert_eq!(eff.len(), 2, "both local subscriptions must stay visible");
    let v = eff.iter().find(|s| s.id == "google_vertex:dev@vertex.test").unwrap();
    let c = eff.iter().find(|s| s.id == "magpie_codex:team007").unwrap();
    assert!(!v.active);
    assert!(c.active, "active_provider from server state must highlight matching local card");
}

#[test]
fn bootstrap_priority_and_legacy_provider_request_mapping() {
    let codex = sample_codex_item();
    let vertex = sample_vertex_item();
    let ordered = pick_ordered_bootstrap_candidates(&[codex.clone(), vertex.clone()]);
    assert_eq!(ordered[0].account.id, vertex.account.id);
    assert_eq!(ordered[1].account.id, codex.account.id);

    let v_req: MagpieSaveProviderRequest = build_legacy_save_provider_request(
        &vertex,
        "ya29.fresh-access-token".to_string(),
        Some("gemini-2.5-pro"),
        true,
    );
    assert_eq!(v_req.api_mode.as_deref(), Some("google_vertex"));
    assert_eq!(v_req.region_id.as_deref(), Some("global"));
    assert_eq!(
        v_req.custom_headers.get("x-goog-user-project").map(String::as_str),
        Some("proj-demo-123")
    );
    assert_eq!(v_req.default_model.as_deref(), Some("gemini-2.5-pro"));
    assert!(v_req.activate);
}

#[test]
fn legacy_v130_server_404_sync_gracefully_falls_back_to_providers_upsert_and_activate() {
    let saved_requests: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let saved_clone = Arc::clone(&saved_requests);

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let base_url = rt.block_on(spawn_mock_http_server(move |method, path, body| {
        if method == "POST" && path == "/api/v1/hermes/magpie/subscriptions/sync" {
            // Simulate legacy v1.3.0 server that doesn't have /subscriptions/sync
            return (404, json!({"error": "not_found"}).to_string());
        }
        if method == "GET" && path == "/api/v1/hermes/magpie/state" {
            return (
                200,
                json!({
                    "active_provider": "local_hermes",
                    "active_model": "hermes-local-v1",
                    "needs_provider_selection": true,
                    "subscriptions": [],
                    "providers": [],
                    "presets": []
                })
                .to_string(),
            );
        }
        if method == "POST" && path == "/api/v1/hermes/magpie/providers" {
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let pid = parsed["provider_id"].as_str().unwrap_or("").to_string();
            let model = parsed["default_model"].as_str().unwrap_or("").to_string();
            saved_clone.lock().unwrap().push(parsed);
            return (
                200,
                json!({
                    "ok": true,
                    "provider": {
                        "provider_id": pid,
                        "name": "Magpie Codex",
                        "api_mode": "codex_responses",
                        "default_model": model,
                        "enabled": true,
                        "has_api_key": true,
                        "is_active": true
                    },
                    "state": {
                        "active_provider": pid,
                        "active_model": model,
                        "needs_provider_selection": false,
                        "has_configured_provider": true,
                        "subscriptions": [],
                        "providers": [],
                        "presets": []
                    }
                })
                .to_string(),
            );
        }
        if method == "POST" && path.contains("/subscriptions/") && path.ends_with("/activate") {
            return (404, json!({"error": "subscription_not_found"}).to_string());
        }
        (404, json!({"error": "unhandled"}).to_string())
    }));

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url.clone();
    let client = Arc::new(TelegramClient::new(cfg));

    // 1. Auto-bootstrap sync falls back to POST /api/v1/hermes/magpie/providers
    let codex = sample_codex_item();
    let outcome = rt.block_on(sync_local_subscriptions_to_server(
        &client,
        &base_url,
        "test-token",
        vec![codex.clone()],
        true,
    ));
    assert!(outcome.used_legacy_fallback);
    assert_eq!(outcome.auto_activated_id.as_deref(), Some("magpie_codex:team007"));
    let st = outcome.state.expect("updated state after fallback save");
    assert_eq!(st.active_provider, "magpie_codex:team007");
    assert!(!st.needs_provider_selection);

    // 2. Explicit activate with model override also works via legacy fallback when /activate returns 404
    let act = rt
        .block_on(activate_subscription_with_local_fallback(
            &client,
            &base_url,
            "test-token",
            "magpie_codex:team007",
            Some("gpt-4o"),
            &[codex],
        ))
        .expect("legacy activate fallback succeeds");
    assert!(act.used_legacy_fallback);
    assert_eq!(act.active_provider_id, "magpie_codex:team007");
    assert_eq!(act.active_model, "gpt-4o");

    let calls = saved_requests.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["default_model"].as_str(), Some("gpt-4o"));
    assert_eq!(
        calls[1]["custom_headers"]["ChatGPT-Account-ID"].as_str(),
        Some("acct_team_007")
    );
    drop(client);
    drop(rt);
}

#[test]
fn native_sync_endpoint_200_uses_first_class_path_without_legacy_fallback() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let base_url = rt.block_on(spawn_mock_http_server(move |method, path, _body| {
        if method == "POST" && path == "/api/v1/hermes/magpie/subscriptions/sync" {
            return (
                200,
                json!({
                    "ok": true,
                    "synced_count": 1,
                    "state": {
                        "active_provider": "google_vertex:dev@vertex.test",
                        "active_model": "gemini-2.5-flash",
                        "needs_provider_selection": false,
                        "has_active_subscription": true,
                        "subscriptions": [],
                        "providers": [],
                        "presets": []
                    }
                })
                .to_string(),
            );
        }
        (404, json!({"error": "unexpected"}).to_string())
    }));

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url.clone();
    let client = Arc::new(TelegramClient::new(cfg));

    let outcome = rt.block_on(sync_local_subscriptions_to_server(
        &client,
        &base_url,
        "test-token",
        vec![sample_vertex_item()],
        true,
    ));
    assert!(!outcome.used_legacy_fallback);
    assert_eq!(
        outcome.auto_activated_id.as_deref(),
        Some("google_vertex:dev@vertex.test")
    );
    assert_eq!(
        outcome.auto_activated_model.as_deref(),
        Some("gemini-2.5-flash")
    );
    drop(client);
    drop(rt);
}

#[test]
fn real_host_local_discovery_smoke_check() {
    if let Ok(home) = std::env::var("HOME") {
        let items = discover_syncable_subscriptions_in_home(
            std::path::Path::new(&home),
            None,
            true,
        );
        let vertex_count = items
            .iter()
            .filter(|i| i.account.kind == SubscriptionKind::GoogleVertex)
            .count();
        assert!(
            vertex_count <= 1,
            "duplicate Google Vertex AI entries must be merged into a single card, found {vertex_count}"
        );
        // Verify no raw secrets leak, no loopback URLs remain, rich models are populated,
        // non-empty display labels exist on every card, and no 'Magpie' branding leaks into labels
        for item in &items {
            assert!(
                !item.account.label.trim().is_empty(),
                "subscription card {} must have a non-empty title",
                item.account.id
            );
            let min_models = match item.account.kind {
                SubscriptionKind::GoogleVertex
                | SubscriptionKind::CodexChatGpt
                | SubscriptionKind::GeminiCodeAssist => 10,
                _ => 3,
            };
            assert!(
                item.available_models.len() >= min_models,
                "subscription card {} should expose full model catalog (>= {min_models}), got {:?}",
                item.account.id,
                item.available_models
            );
            assert!(
                !item.account.label.to_ascii_lowercase().contains("magpie"),
                "user-facing label must not contain Magpie: {}",
                item.account.label
            );
            if let Some(ref plan) = item.account.plan {
                assert!(
                    !plan.to_ascii_lowercase().contains("magpie"),
                    "user-facing plan must not contain Magpie: {plan}"
                );
            }
            if let Some(ref url) = item.account.base_url {
                assert!(
                    !magpie_gateway::is_loopback_server_url(url),
                    "remote sync item {} must not retain loopback URL {url}",
                    item.account.id
                );
            }
        }
    }
}

#[test]
fn composer_chinese_text_does_not_open_sticker_popup_and_allows_enter_send() {
    use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
    use gpui_telegram_desktop::views::composer::ime_machine::KeyAction;

    let mut comp = CanonicalComposer::new();
    comp.suggest_stickers_by_emoji = true;
    comp.ime_machine.insert_text("你好");
    comp.sync_autocomplete();
    assert!(
        !comp.autocomplete.is_open,
        "Chinese text like '你好' must NEVER trigger the Sticker autocomplete popup"
    );
    assert_eq!(comp.handle_enter(false), KeyAction::SendMessage);

    // Real emoji still opens Sticker autocomplete
    let mut comp_emoji = CanonicalComposer::new();
    comp_emoji.suggest_stickers_by_emoji = true;
    comp_emoji.ime_machine.insert_text("😀");
    comp_emoji.sync_autocomplete();
    assert!(
        comp_emoji.autocomplete.is_open,
        "Actual emoji '😀' should still open Sticker suggestions"
    );
}

#[test]
fn e2ee_vault_bundle_excludes_loopback_proxies_and_rotating_oauth_on_push_and_pull() {
    use bot_browser::e2ee_sync::AiOauthVaultBundle;
    use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
    use gpui_telegram_agent::hermes::providers::{ApiMode, ProviderConfig, ProviderRouter};
    use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::{
        build_ai_oauth_vault_bundle, hydrate_ai_oauth_vault_bundle_into_fabushi_for_user,
    };

    // 1. Push side: build_ai_oauth_vault_bundle must strip Codex rotating OAuth, loopback Claude proxy,
    // and ephemeral Gemini Code Assist while keeping static API key & Vertex ADC.
    let router = ProviderRouter::new();
    router.register_provider(ProviderConfig::openai_compatible(
        "openai_static",
        "OpenAI Static",
        "https://api.openai.com/v1",
        "gpt-4o",
        "sk-static-cross-device",
    ));
    let mut loopback_claude = ProviderConfig::openai_compatible(
        "claude_code:settings",
        "Claude Code (Settings)",
        "http://127.0.0.1:3425/anthropic",
        "codex/gpt-6.1-sol",
        "local-proxy-token",
    );
    loopback_claude.api_mode = ApiMode::AnthropicMessages;
    loopback_claude.is_subscription = true;
    router.register_provider(loopback_claude);

    let mut codex_cfg = ProviderConfig::openai_compatible(
        "magpie_codex:team007",
        "Codex",
        "https://chatgpt.com/backend-api/codex",
        "codex-mini-latest",
        "eyJ-codex",
    );
    codex_cfg.api_mode = ApiMode::CodexResponses;
    codex_cfg.is_subscription = true;
    router.register_provider(codex_cfg);
    let _ = router.set_active("magpie_codex:team007", Some("codex-mini-latest"));

    let mut gemini_item = sample_codex_item();
    gemini_item.account.id = "magpie_gemini:abc".to_string();
    gemini_item.account.kind = SubscriptionKind::GeminiCodeAssist;
    gemini_item.account.base_url = Some("https://cloudcode-pa.googleapis.com/v1internal".to_string());

    let bundle = build_ai_oauth_vault_bundle(
        &[sample_vertex_item(), sample_codex_item(), gemini_item.clone()],
        Some(&router),
    )
    .expect("build vault bundle");

    let subs: Vec<ClientSubscriptionSyncItem> =
        serde_json::from_value(bundle.subscriptions_json.clone()).unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].account.id, "google_vertex:dev@vertex.test");

    let providers_text = bundle.providers_json.to_string();
    assert!(providers_text.contains("openai_static"));
    assert!(!providers_text.contains("magpie_codex:team007"));
    assert!(!providers_text.contains("claude_code:settings"));
    assert!(!providers_text.contains("127.0.0.1:3425"));

    // 2. Pull side: even a legacy vault bundle uploaded by an older client containing raw
    // Codex OAuth + loopback proxy + Gemini Code Assist must be sanitized on hydrate!
    let legacy_bundle = AiOauthVaultBundle {
        providers_json: serde_json::to_value(router.snapshot()).unwrap(),
        subscriptions_json: serde_json::to_value(vec![sample_codex_item(), gemini_item]).unwrap(),
        updated_at_ms: 1_700_000_000_000,
    };
    let host_b = FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap();
    let cnt = hydrate_ai_oauth_vault_bundle_into_fabushi_for_user(
        &host_b,
        &legacy_bundle,
        Some("2002"),
    )
    .expect("hydrate sanitized legacy bundle");
    assert!(cnt >= 1);
    let snap_b = host_b.core_engine().providers_for_user("2002").snapshot();
    assert!(snap_b.providers.contains_key("openai_static"));
    assert!(!snap_b.providers.contains_key("magpie_codex:team007"));
    assert!(!snap_b.providers.contains_key("claude_code:settings"));
    assert!(!snap_b.providers.contains_key("magpie_gemini:abc"));
    assert!(snap_b.synced_codex_oauths.is_empty());
    assert_eq!(snap_b.active_provider_id, "openai_static");
}
