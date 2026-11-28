//! `AppShell` coordinator for the unified OpenBot 1:1 Marketplace (`市场`):
//! - Live catalog sync with `gpui_telegram_server` (`/api/v1/marketplace/*`): fetches real
//!   published connectors/plugins/agents/skills, real install counts, team-published custom
//!   connectors, and encrypted cloud connection records.
//! - Live verification of native connectors (`Slack`, `Discord`, `Telegram`, `WhatsApp`,
//!   `1Password`, `Bitwarden`, `Proton Pass`, `X`, `GitHub`) against authentic upstream APIs/CLIs
//!   before ever marking `connected`.
//! - Live Streamable HTTP (`initialize` + `Mcp-Session-Id` + SSE + `tools/list`) & STDIO JSON-RPC
//!   verification for all 257 OpenBot MCP plugins, server-published connectors, and custom servers.
//! - Real OAuth 2.0 RFC 9728 + RFC 8414 + RFC 7591 DCR + PKCE S256 loopback (`127.0.0.1:0/callback`)
//!   authorization flow for `link` MCP servers.
//! - Live agent provisioning & per-agent skill installation synchronized with Bot Inspector.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::{AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_proto::marketplace::{
    ConnectMarketplaceAppRequest, MarketplaceAppListingDto, MarketplaceEnvPairDto,
    MarketplaceToolSchemaDto, PublishMarketplaceAppRequest,
};
use gpui_telegram_proto::miniapp_contracts::CreateFabushiBotRequest;
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::routines::{
    RoutineLimitPolicy, RoutineMissedPolicy, RoutineRunPlace, RoutineSchedule,
};
use openbot_runtime::agent_lifecycle::CreateAgentRequest;
use openbot_runtime::platform::marketplace::{
    MarketplaceTab, OpenBotAgentRoutineSpec, openbot_marketplace_catalog,
};
use openbot_runtime::platform::routines::RoutineSaveRequest;
use openbot_runtime::skill_mcp_service::{
    McpDiscoveredTool, RegisterMcpServerInput, SaveSkillInput, probe_http_mcp_endpoint,
};
use openbot_storage::{McpKeyValue, McpServerConfig, McpTransport, mcp_config_errors};

use crate::i18n::tr_str;
use crate::views::apple_sidebar::AppleSidebarSection;
use crate::views::content_hub::{
    ContentHubInputTarget, ContentHubSection, MarketplaceBotSummary, MarketplaceNavTab,
    MarketplaceRoute,
};
use crate::views::shell::{ActiveWorkspacePanel, AppShell};

fn nearest_avatar_hue(hue_deg: u16) -> AvatarHue {
    let target = hue_deg % 360;
    AvatarHue::ALL
        .into_iter()
        .min_by_key(|h| {
            let d = h.degrees().abs_diff(target);
            d.min(360u16.saturating_sub(d))
        })
        .unwrap_or(AvatarHue::Indigo)
}

fn sanitize_slug_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_' || ch == ' ') && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed
    }
}

fn parse_marketplace_routine_schedule(spec: &OpenBotAgentRoutineSpec) -> RoutineSchedule {
    let time = if spec.schedule_time.trim().len() == 5 && spec.schedule_time.as_bytes()[2] == b':' {
        spec.schedule_time.trim().to_string()
    } else {
        "09:00".to_string()
    };
    match spec.schedule_kind.trim().to_ascii_lowercase().as_str() {
        "hourly" => RoutineSchedule::Hourly { minute: 0 },
        "daily" => RoutineSchedule::Daily { time },
        "weekdays" => RoutineSchedule::Weekdays { time },
        "weekly" => RoutineSchedule::Weekly { weekday: 1, time },
        "monthly" => RoutineSchedule::Monthly { day: 1, time },
        _ => RoutineSchedule::Weekdays { time },
    }
}

fn to_proto_tools(tools: &[McpDiscoveredTool]) -> Vec<MarketplaceToolSchemaDto> {
    tools
        .iter()
        .map(|t| MarketplaceToolSchemaDto {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}

fn is_local_loopback_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://[::1]")
        || lower.starts_with("https://127.0.0.1")
        || lower.starts_with("https://localhost")
}

fn build_probe_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("fabushi-openbot/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl AppShell {
    /// Opens the unified OpenBot Marketplace (`市场`) directly on `tab` (`应用`, `助手`, `技能`,
    /// `小程序应用`, `图书`, or `游戏`) and synchronizes live connector/agent/skill runtime &
    /// cloud backend state.
    pub fn open_marketplace_tab(&mut self, tab: MarketplaceNavTab, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return;
        }
        self.leave_workspace_modal_panel_for_hub(cx);
        self.dismiss_transient_popovers_for_hub(cx);
        self.active_panel = ActiveWorkspacePanel::ContentHub(ContentHubSection::Marketplace);
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        }
        self.apple_sidebar.update(cx, |sb, scx| {
            sb.set_active_section(AppleSidebarSection::Marketplace);
            scx.notify();
        });
        self.content_hub.update(cx, |h, hcx| {
            h.set_active_input_ime_preedit(None);
            h.active_section = ContentHubSection::Marketplace;
            h.marketplace.select_tab(tab);
            h.selected_creation_id = None;
            h.selected_catalog_item = None;
            h.selected_detail_creation = None;
            h.selected_genre_filter = None;
            h.book_reader = None;
            h.active_game = None;
            h.active_reviews.clear();
            h.active_leaderboard.clear();
            h.review_status_notice = None;
            if h.active_input != ContentHubInputTarget::QuickDrawerSearch {
                h.active_input = ContentHubInputTarget::None;
            }
            h.active_sub_tab = match tab {
                MarketplaceNavTab::MiniApps => {
                    ContentHubSection::AppStore.default_sub_tab().to_string()
                }
                MarketplaceNavTab::Books => ContentHubSection::Books.default_sub_tab().to_string(),
                MarketplaceNavTab::Games => ContentHubSection::Games.default_sub_tab().to_string(),
                MarketplaceNavTab::Apps | MarketplaceNavTab::Agents | MarketplaceNavTab::Skills => {
                    ContentHubSection::Marketplace.default_sub_tab().to_string()
                }
            };
            hcx.notify();
        });
        self.sync_local_miniapps_to_content_hub(cx);
        self.sync_marketplace_runtime_state(cx);
        match tab {
            MarketplaceNavTab::MiniApps => {
                self.refresh_content_hub_catalog(ContentHubSection::AppStore, cx);
            }
            MarketplaceNavTab::Books => {
                self.refresh_content_hub_catalog(ContentHubSection::Books, cx);
            }
            MarketplaceNavTab::Games => {
                self.refresh_content_hub_catalog(ContentHubSection::Games, cx);
            }
            MarketplaceNavTab::Apps | MarketplaceNavTab::Agents | MarketplaceNavTab::Skills => {
                self.refresh_marketplace_from_server(cx);
            }
        }
        cx.notify();
    }

    /// Fetches the live SQLite-backed Marketplace catalog, real install counters, user-published
    /// connectors, and verified cloud connection state from `gpui_telegram_server`.
    pub fn refresh_marketplace_from_server(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        let tokio_rt = Arc::clone(self.client.runtime());
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.server_sync_loading = true;
            hcx.notify();
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let client = Arc::clone(&fence.client);
            let token = fence.token.clone();
            let epoch = fence.client_epoch;
            let result = tokio_rt
                .spawn(async move { client.get_marketplace_catalog_with_token(&token, epoch).await })
                .await;

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                match result {
                    Ok(Ok(resp)) => {
                        let static_cat = openbot_marketplace_catalog();
                        let static_ids: HashSet<&str> = static_cat
                            .connectors
                            .iter()
                            .map(|c| c.id.as_str())
                            .chain(static_cat.plugins.iter().map(|p| p.id.as_str()))
                            .collect();

                        let mut app_counts = BTreeMap::new();
                        let mut published_custom: Vec<MarketplaceAppListingDto> = Vec::new();
                        for listing in resp.apps {
                            app_counts.insert(listing.app_id.clone(), listing.install_count);
                            if !static_ids.contains(listing.app_id.as_str()) {
                                published_custom.push(listing);
                            }
                        }

                        let mut agent_counts = BTreeMap::new();
                        for a in resp.agents {
                            agent_counts.insert(a.slug, a.install_count);
                        }
                        let mut skill_counts = BTreeMap::new();
                        for s in resp.skills {
                            skill_counts.insert(s.slug, s.install_count);
                        }
                        let mut server_conns = BTreeMap::new();
                        for c in resp.connections {
                            server_conns.insert(c.app_id.clone(), c);
                        }

                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.published_custom_apps = published_custom;
                            h.marketplace.live_app_install_counts = app_counts;
                            h.marketplace.live_agent_install_counts = agent_counts;
                            h.marketplace.live_skill_install_counts = skill_counts;
                            h.marketplace.server_connections = server_conns;
                            h.marketplace.server_total_apps = resp.total_apps;
                            h.marketplace.server_synced = true;
                            h.marketplace.server_sync_loading = false;
                            hcx.notify();
                        });
                    }
                    _ => {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.server_sync_loading = false;
                            hcx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Synchronizes live `OpenBotHostRuntime` connectors, MCP servers, per-server agent access,
    /// installed agent slugs, and installed skills into `ContentHubWorkspace::marketplace`.
    pub fn sync_marketplace_runtime_state(&mut self, cx: &mut Context<Self>) {
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let cached_bots = self.miniapps.fabushi_bots.clone();
        for fab in &cached_bots {
            let _ = self.provision_openbot_agent_for_fabushi(fab, &[]);
        }

        let connected_apps = rt.marketplace().list_connected_apps();
        let mcp_servers = rt.mcp().list_servers().unwrap_or_default();
        let server_access = rt.mcp().list_server_access();
        let agents = rt.agents().list_agents();

        let mut added_agent_slugs: HashSet<String> = rt
            .marketplace()
            .list_installed_slugs(MarketplaceTab::Agents)
            .into_iter()
            .collect();

        let mut bots = Vec::with_capacity(agents.len());
        for a in &agents {
            if let Some(slug) = a.title.strip_prefix("marketplace:") {
                added_agent_slugs.insert(slug.to_string());
            }
            let fab_match = a
                .title
                .strip_prefix("fabushi:")
                .and_then(|aid| {
                    self.miniapps
                        .fabushi_bots
                        .iter()
                        .find(|b| b.agent_id == aid)
                });
            let installed_skill_slugs: Vec<String> = rt
                .skills()
                .list_skills(&a.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.enabled)
                .map(|s| s.slug)
                .collect();
            bots.push(MarketplaceBotSummary {
                openbot_agent_id: a.id.clone(),
                fabushi_agent_id: fab_match.map(|f| f.agent_id.clone()),
                username: fab_match.map(|f| f.username.clone()),
                name: a.name.clone(),
                title: fab_match
                    .map(|f| f.title.clone())
                    .unwrap_or_else(|| a.title.clone()),
                avatar_shape: fab_match
                    .map(|f| f.avatar_shape.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| a.avatar_seed.clone()),
                avatar_color: fab_match
                    .map(|f| f.avatar_color.clone())
                    .filter(|s| !s.is_empty())
                    .or_else(|| a.avatar_color.clone().flatten())
                    .unwrap_or_default(),
                installed_skill_slugs,
            });
        }

        for fab in &self.miniapps.fabushi_bots {
            if let Some(slug) = fab.agent_id.strip_prefix("ob_") {
                added_agent_slugs.insert(slug.replace('_', "-"));
                added_agent_slugs.insert(slug.to_string());
            }
        }

        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.connected_apps = connected_apps;
            h.marketplace.mcp_servers = mcp_servers;
            h.marketplace.server_access = server_access;
            h.marketplace.bots = bots;
            h.marketplace.added_agent_slugs = added_agent_slugs;
            hcx.notify();
        });
    }

    /// Real live MCP verification for an upstream OpenBot plugin (`initialize` + `Mcp-Session-Id` +
    /// `notifications/initialized` + `tools/list`), then syncs verified connection & discovered tools
    /// to `gpui_telegram_server` so cloud Fabushi Bots immediately gain live executable MCP tools.
    pub fn handle_marketplace_connect_plugin(
        &mut self,
        slug: String,
        auth_values: BTreeMap<String, String>,
        override_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let app_id = openbot_marketplace_catalog()
            .plugins
            .iter()
            .find(|p| p.slug == slug)
            .map(|p| p.id.clone())
            .unwrap_or_else(|| format!("plugin:{slug}"));

        let tokio_rt = Arc::clone(self.client.runtime());
        let fence = self.capture_client_fence();
        let app_id_pending = app_id.clone();

        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.connecting_app_id = Some(app_id_pending);
            h.marketplace.error_banner = None;
            h.marketplace.notice_banner = Some(
                tr_str(
                    &lang,
                    "正在执行真实 MCP 握手与工具探测…",
                    "Running live MCP handshake & tool discovery…",
                )
                .to_string(),
            );
            if let Some(ref mut d) = h.marketplace.setup_draft {
                d.error = None;
                d.notice = None;
            }
            hcx.notify();
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let slug_for_probe = slug.clone();
            let auth_for_probe = auth_values.clone();
            let override_for_probe = override_url.clone();
            let rt_bg = Arc::clone(&rt);

            let local_outcome = tokio_rt
                .spawn(async move {
                    rt_bg
                        .marketplace()
                        .connect_plugin_via_mcp_verified(
                            rt_bg.mcp(),
                            &slug_for_probe,
                            &auth_for_probe,
                            override_for_probe.as_deref(),
                        )
                        .await
                })
                .await;

            match local_outcome {
                Ok(Ok((rec, saved_srv, probe))) => {
                    if probe.status == "ready" {
                        // Sync verified connection + discovered tools to cloud backend when signed in.
                        let mut server_conn_opt = None;
                        let mut server_installs = None;
                        if let Some(ref f) = fence {
                            let is_local_or_stdio = saved_srv.transport == McpTransport::Stdio
                                || is_local_loopback_url(&saved_srv.url);
                            let req = ConnectMarketplaceAppRequest {
                                app_id: rec.app_id.clone(),
                                slug: Some(slug.clone()),
                                kind: Some("plugin".to_string()),
                                name: Some(rec.name.clone()),
                                account_label: rec.account_label.clone(),
                                credential: auth_values
                                    .get("oauth_token")
                                    .cloned(),
                                auth_values: auth_values.clone(),
                                override_url: if saved_srv.transport == McpTransport::Http {
                                    Some(saved_srv.url.clone())
                                } else {
                                    override_url.clone()
                                },
                                transport: Some(match saved_srv.transport {
                                    McpTransport::Http => "http".to_string(),
                                    McpTransport::Stdio => "stdio".to_string(),
                                }),
                                command: (!saved_srv.command.is_empty())
                                    .then_some(saved_srv.command.clone()),
                                args: saved_srv.args.clone(),
                                headers: saved_srv
                                    .headers
                                    .iter()
                                    .map(|kv| MarketplaceEnvPairDto {
                                        key: kv.key.clone(),
                                        value: kv.value.clone(),
                                    })
                                    .collect(),
                                metadata: rec.metadata.clone(),
                                // Already verified live on client; cloud server stores encrypted creds
                                // and registers live HTTP tool handlers using verified schemas without
                                // failing on client-only stdio or firewall-restricted endpoints.
                                verify_live: !is_local_or_stdio && probe.tool_defs.is_empty(),
                                local_discovered_tools: to_proto_tools(&probe.tool_defs),
                            };
                            let client = Arc::clone(&f.client);
                            let tok = f.token.clone();
                            let ep = f.client_epoch;
                            if let Ok(Ok(resp)) = tokio_rt
                                .spawn(async move {
                                    client
                                        .connect_marketplace_app_with_token(&tok, ep, &req)
                                        .await
                                })
                                .await
                            {
                                server_conn_opt = Some(resp.connection);
                                server_installs = Some(resp.install_count);
                            }
                        }

                        let tool_n = probe.tool_count;
                        let msg = if lang.starts_with("zh") {
                            format!("已验证并连接 {}（发现 {} 个实时工具）", rec.name, tool_n)
                        } else {
                            format!("Verified & connected {} ({} live tools)", rec.name, tool_n)
                        };
                        let _ = weak.update(app, |shell, cx| {
                            shell.sync_marketplace_runtime_state(cx);
                            shell.refresh_active_bot_fusion_snapshot(cx);
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.connecting_app_id = None;
                                if let Some(sc) = server_conn_opt {
                                    h.marketplace
                                        .server_connections
                                        .insert(sc.app_id.clone(), sc);
                                }
                                if let Some(cnt) = server_installs {
                                    h.marketplace
                                        .live_app_install_counts
                                        .insert(rec.app_id.clone(), cnt);
                                }
                                h.marketplace.error_banner = None;
                                h.marketplace.notice_banner = Some(msg.clone());
                                if let Some(ref mut d) = h.marketplace.setup_draft {
                                    d.error = None;
                                    d.notice = Some(msg.clone());
                                }
                                hcx.notify();
                            });
                            shell.show_non_chat_toast(msg, 3000, cx);
                            cx.notify();
                        });
                    } else if probe.status == "needs_sign_in" {
                        let notice = if lang.starts_with("zh") {
                            format!(
                                "{} 需要浏览器 OAuth 2.0 授权（HTTP 401）。请点击「浏览器 OAuth 登录」完成授权，或粘贴有效访问令牌。",
                                rec.name
                            )
                        } else {
                            format!(
                                "{} requires OAuth 2.0 browser sign-in (HTTP 401). Click \"Sign in with Browser (OAuth)\" or paste a valid Bearer token.",
                                rec.name
                            )
                        };
                        let _ = weak.update(app, |shell, cx| {
                            shell.sync_marketplace_runtime_state(cx);
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.connecting_app_id = None;
                                h.marketplace.notice_banner = None;
                                h.marketplace.error_banner = Some(notice.clone());
                                if let Some(ref mut d) = h.marketplace.setup_draft {
                                    d.error = Some(notice.clone());
                                    d.notice = None;
                                }
                                hcx.notify();
                            });
                            cx.notify();
                        });
                    } else {
                        let detail = probe.error.unwrap_or_else(|| {
                            tr_str(
                                &lang,
                                "无法与该 MCP 服务器完成握手或凭据验证失败。",
                                "Live MCP handshake or credential verification failed.",
                            )
                            .to_string()
                        });
                        let _ = weak.update(app, |shell, cx| {
                            shell.sync_marketplace_runtime_state(cx);
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.connecting_app_id = None;
                                h.marketplace.notice_banner = None;
                                h.marketplace.error_banner = Some(detail.clone());
                                if let Some(ref mut d) = h.marketplace.setup_draft {
                                    d.error = Some(detail);
                                    d.notice = None;
                                }
                                hcx.notify();
                            });
                            cx.notify();
                        });
                    }
                }
                Ok(Err(err)) => {
                    let err_msg = err.to_string();
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.notice_banner = None;
                            h.marketplace.error_banner = Some(err_msg.clone());
                            if let Some(ref mut d) = h.marketplace.setup_draft {
                                d.error = Some(err_msg);
                            }
                            hcx.notify();
                        });
                        cx.notify();
                    });
                }
                Err(join_err) => {
                    let err_msg = format!("MCP probe task error: {join_err}");
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.error_banner = Some(err_msg);
                            hcx.notify();
                        });
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Connects a real server-published custom connector (`MarketplaceAppEntry::Published`) via live
    /// MCP verification on the server (for remote HTTP) or local STDIO probe, then updates state.
    pub fn handle_marketplace_connect_published(
        &mut self,
        app_id: String,
        auth_values: BTreeMap<String, String>,
        override_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let Some(fence) = self.capture_client_fence() else {
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.error_banner = Some(
                    tr_str(&lang, "请先登录后再连接已上架应用。", "Please sign in first.")
                        .to_string(),
                );
                hcx.notify();
            });
            return;
        };
        let listing = self
            .content_hub
            .read(cx)
            .marketplace
            .published_custom_apps
            .iter()
            .find(|a| a.app_id == app_id)
            .cloned();
        let Some(listing) = listing else {
            return;
        };

        let tokio_rt = Arc::clone(self.client.runtime());
        let app_id_pending = app_id.clone();
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.connecting_app_id = Some(app_id_pending);
            h.marketplace.error_banner = None;
            h.marketplace.notice_banner = Some(
                tr_str(
                    &lang,
                    "正在验证并连接云端已上架连接器…",
                    "Verifying & connecting published connector…",
                )
                .to_string(),
            );
            hcx.notify();
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let req = ConnectMarketplaceAppRequest {
                app_id: listing.app_id.clone(),
                slug: Some(listing.slug.clone()),
                kind: Some(listing.kind.clone()),
                name: Some(listing.name.clone()),
                account_label: None,
                credential: auth_values.get("oauth_token").cloned(),
                auth_values,
                override_url: override_url.or_else(|| listing.url.clone()),
                transport: Some(listing.transport.clone()),
                command: listing.command.clone(),
                args: listing.args.clone(),
                headers: Vec::new(),
                metadata: BTreeMap::new(),
                verify_live: listing.transport.eq_ignore_ascii_case("http"),
                local_discovered_tools: Vec::new(),
            };
            let client = Arc::clone(&fence.client);
            let tok = fence.token.clone();
            let ep = fence.client_epoch;
            let res = tokio_rt
                .spawn(async move {
                    client
                        .connect_marketplace_app_with_token(&tok, ep, &req)
                        .await
                })
                .await;

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                match res {
                    Ok(Ok(resp)) => {
                        let is_connected = resp.connection.status == "connected";
                        let tool_n = resp.connection.discovered_tools.len();
                        let app_name = resp.connection.name.clone();
                        let last_err = resp.connection.last_error.clone();
                        let requires_oauth = resp.requires_oauth;
                        let msg = if is_connected {
                            if lang.starts_with("zh") {
                                format!("已验证并连接 {}（发现 {} 个实时工具）", app_name, tool_n)
                            } else {
                                format!("Verified & connected {} ({} live tools)", app_name, tool_n)
                            }
                        } else if requires_oauth {
                            if lang.starts_with("zh") {
                                format!(
                                    "{app_name} 需要有效的访问令牌或 OAuth 授权（HTTP 401）。请填入访问令牌后重试。"
                                )
                            } else {
                                format!(
                                    "{app_name} requires a valid Bearer token or OAuth sign-in (HTTP 401)."
                                )
                            }
                        } else {
                            last_err.unwrap_or_else(|| {
                                format!("Connection verification status: {}", resp.connection.status)
                            })
                        };
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace
                                .live_app_install_counts
                                .insert(resp.connection.app_id.clone(), resp.install_count);
                            h.marketplace
                                .server_connections
                                .insert(resp.connection.app_id.clone(), resp.connection);
                            if is_connected {
                                h.marketplace.error_banner = None;
                                h.marketplace.notice_banner = Some(msg.clone());
                                if let Some(ref mut d) = h.marketplace.setup_draft {
                                    d.error = None;
                                    d.notice = Some(msg.clone());
                                }
                            } else {
                                h.marketplace.notice_banner = None;
                                h.marketplace.error_banner = Some(msg.clone());
                                if let Some(ref mut d) = h.marketplace.setup_draft {
                                    d.error = Some(msg.clone());
                                    d.notice = None;
                                }
                            }
                            hcx.notify();
                        });
                        if is_connected {
                            shell.show_non_chat_toast(msg, 3000, cx);
                        }
                    }
                    Ok(Err(err)) => {
                        let msg = err.to_string();
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.notice_banner = None;
                            h.marketplace.error_banner = Some(msg.clone());
                            if let Some(ref mut d) = h.marketplace.setup_draft {
                                d.error = Some(msg);
                            }
                            hcx.notify();
                        });
                    }
                    Err(join_err) => {
                        let msg = format!("Connect task failed: {join_err}");
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.error_banner = Some(msg);
                            hcx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs the real OAuth 2.0 RFC 9728 + RFC 8414 + RFC 7591 DCR + PKCE S256 loopback callback
    /// flow (`127.0.0.1:0/callback`), opens the vendor sign-in URL in the OS browser, exchanges
    /// the real authorization code, probes `tools/list`, and syncs the verified token to the server.
    pub fn handle_marketplace_start_oauth(
        &mut self,
        slug: String,
        override_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let plugin = openbot_marketplace_catalog()
            .plugins
            .iter()
            .find(|p| p.slug == slug);
        let Some(plugin) = plugin else {
            return;
        };
        let Some(mcp_url) = override_url
            .filter(|s| !s.trim().is_empty())
            .or_else(|| plugin.url.clone())
        else {
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.error_banner = Some(
                    tr_str(
                        &lang,
                        "该插件未配置远程 OAuth MCP 端点。",
                        "This plugin does not expose a remote HTTP OAuth MCP endpoint.",
                    )
                    .to_string(),
                );
                hcx.notify();
            });
            return;
        };

        // Ensure a local MCP server registration exists so OAuth tokens bind to a real server_id.
        let server_id = match rt
            .marketplace()
            .connect_plugin_via_mcp(rt.mcp(), &slug, &BTreeMap::new(), Some(&mcp_url))
        {
            Ok((rec, srv)) => {
                // Keep status as `needs_sign_in` until OAuth actually completes & verifies!
                let _ = rt.marketplace().update_connection_probe_state(
                    &rec.app_id,
                    "needs_sign_in",
                    Vec::new(),
                    Some("Waiting for browser OAuth authorization".to_string()),
                );
                srv.id
            }
            Err(err) => {
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = Some(err.to_string());
                    hcx.notify();
                });
                return;
            }
        };

        let app_id = plugin.id.clone();
        let app_name = plugin.name.clone();
        let tokio_rt = Arc::clone(self.client.runtime());
        let fence = self.capture_client_fence();

        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.connecting_app_id = Some(app_id.clone());
            h.marketplace.error_banner = None;
            h.marketplace.notice_banner = Some(
                tr_str(
                    &lang,
                    "正在发现 OAuth 2.0 授权端点并启动本地回调服务…",
                    "Discovering OAuth 2.0 endpoints and starting loopback callback…",
                )
                .to_string(),
            );
            hcx.notify();
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let mcp_arc = Arc::clone(rt.mcp());
            let sid = server_id.clone();
            let url_for_oauth = mcp_url.clone();
            let started = tokio_rt
                .spawn(async move {
                    mcp_arc
                        .start_loopback_oauth_flow(&sid, &url_for_oauth, None)
                        .await
                })
                .await;

            let (authorize_url, callback_handle) = match started {
                Ok(Ok(pair)) => pair,
                Ok(Err(err)) => {
                    let msg = err.to_string();
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.notice_banner = None;
                            h.marketplace.error_banner = Some(msg.clone());
                            if let Some(ref mut d) = h.marketplace.setup_draft {
                                d.error = Some(msg);
                            }
                            hcx.notify();
                        });
                        cx.notify();
                    });
                    return;
                }
                Err(join_err) => {
                    let msg = format!("OAuth setup task failed: {join_err}");
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.error_banner = Some(msg);
                            hcx.notify();
                        });
                        cx.notify();
                    });
                    return;
                }
            };

            // Open the real vendor authorization URL in the user's browser.
            let auth_url_open = authorize_url.clone();
            let waiting_msg = if lang.starts_with("zh") {
                format!("已在浏览器打开 {app_name} OAuth 授权页，等待回调完成…")
            } else {
                format!("Opened {app_name} OAuth sign-in in browser — waiting for callback…")
            };
            let _ = weak.update(app, |shell, cx| {
                shell.open_external_url_with_toast(&auth_url_open, cx);
                shell.content_hub.update(cx, |h, hcx| {
                    h.marketplace.notice_banner = Some(waiting_msg.clone());
                    if let Some(ref mut d) = h.marketplace.setup_draft {
                        d.notice = Some(waiting_msg);
                    }
                    hcx.notify();
                });
                cx.notify();
            });

            // Await the real OAuth loopback callback + PKCE token exchange.
            let token_res = callback_handle.await;
            match token_res {
                Ok(Ok(tok_resp)) => {
                    let rt_probe = Arc::clone(&rt);
                    let sid_probe = server_id.clone();
                    let probe_res = tokio_rt
                        .spawn(async move { rt_probe.mcp().probe_server(&sid_probe).await })
                        .await;

                    if let Ok(Ok(probe)) = probe_res {
                        if probe.status == "ready" {
                            let _ = rt.marketplace().update_connection_probe_state(
                                &app_id,
                                "connected",
                                probe.tool_defs.clone(),
                                None,
                            );
                            // Sync verified OAuth token & discovered tools to cloud backend so bots can call it!
                            let mut server_conn_opt = None;
                            let mut server_installs = None;
                            if let Some(ref f) = fence {
                                let mut auth_map = BTreeMap::new();
                                auth_map.insert(
                                    "oauth_token".to_string(),
                                    tok_resp.access_token.clone(),
                                );
                                let req = ConnectMarketplaceAppRequest {
                                    app_id: app_id.clone(),
                                    slug: Some(slug.clone()),
                                    kind: Some("plugin".to_string()),
                                    name: Some(app_name.clone()),
                                    account_label: Some(mcp_url.clone()),
                                    credential: Some(tok_resp.access_token.clone()),
                                    auth_values: auth_map,
                                    override_url: Some(mcp_url.clone()),
                                    transport: Some("http".to_string()),
                                    command: None,
                                    args: Vec::new(),
                                    headers: Vec::new(),
                                    metadata: BTreeMap::new(),
                                    verify_live: false,
                                    local_discovered_tools: to_proto_tools(&probe.tool_defs),
                                };
                                let client = Arc::clone(&f.client);
                                let tok = f.token.clone();
                                let ep = f.client_epoch;
                                if let Ok(Ok(resp)) = tokio_rt
                                    .spawn(async move {
                                        client
                                            .connect_marketplace_app_with_token(&tok, ep, &req)
                                            .await
                                    })
                                    .await
                                {
                                    server_conn_opt = Some(resp.connection);
                                    server_installs = Some(resp.install_count);
                                }
                            }
                            let msg = if lang.starts_with("zh") {
                                format!(
                                    "OAuth 授权并验证成功：{}（发现 {} 个实时工具）",
                                    app_name, probe.tool_count
                                )
                            } else {
                                format!(
                                    "OAuth connected {}: {} live tools discovered",
                                    app_name, probe.tool_count
                                )
                            };
                            let _ = weak.update(app, |shell, cx| {
                                shell.sync_marketplace_runtime_state(cx);
                                shell.refresh_active_bot_fusion_snapshot(cx);
                                shell.content_hub.update(cx, |h, hcx| {
                                    h.marketplace.connecting_app_id = None;
                                    if let Some(sc) = server_conn_opt {
                                        h.marketplace
                                            .server_connections
                                            .insert(sc.app_id.clone(), sc);
                                    }
                                    if let Some(cnt) = server_installs {
                                        h.marketplace
                                            .live_app_install_counts
                                            .insert(app_id.clone(), cnt);
                                    }
                                    h.marketplace.error_banner = None;
                                    h.marketplace.notice_banner = Some(msg.clone());
                                    hcx.notify();
                                });
                                shell.show_non_chat_toast(msg, 3200, cx);
                                cx.notify();
                            });
                            return;
                        }
                        let err_msg = probe.error.unwrap_or_else(|| {
                            "OAuth succeeded but MCP tools/list verification failed.".to_string()
                        });
                        let _ = rt.marketplace().update_connection_probe_state(
                            &app_id,
                            &probe.status,
                            Vec::new(),
                            Some(err_msg.clone()),
                        );
                        let _ = weak.update(app, |shell, cx| {
                            shell.sync_marketplace_runtime_state(cx);
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.connecting_app_id = None;
                                h.marketplace.notice_banner = None;
                                h.marketplace.error_banner = Some(err_msg);
                                hcx.notify();
                            });
                            cx.notify();
                        });
                    }
                }
                Ok(Err(err)) => {
                    let msg = err.to_string();
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.notice_banner = None;
                            h.marketplace.error_banner = Some(msg.clone());
                            if let Some(ref mut d) = h.marketplace.setup_draft {
                                d.error = Some(msg);
                            }
                            hcx.notify();
                        });
                        cx.notify();
                    });
                }
                Err(join_err) => {
                    let msg = format!("OAuth callback wait failed: {join_err}");
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.error_banner = Some(msg);
                            hcx.notify();
                        });
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Real live verification for native connectors (`Slack`, `Discord`, `Telegram`, `WhatsApp`,
    /// `GitHub`, `X`, `1Password`, `Bitwarden`, `Proton Pass`). Never marks connected unless the
    /// live upstream API or local vault CLI verifies the supplied credentials.
    pub fn handle_marketplace_connect_native(
        &mut self,
        connector_id: String,
        account_label: Option<String>,
        credential: Option<String>,
        metadata: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let app_id = format!("connector:{connector_id}");
        let tokio_rt = Arc::clone(self.client.runtime());
        let fence = self.capture_client_fence();
        let is_vault_cli = matches!(
            connector_id.as_str(),
            "onepassword" | "bitwarden" | "protonpass"
        );

        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.connecting_app_id = Some(app_id.clone());
            h.marketplace.error_banner = None;
            h.marketplace.notice_banner = Some(
                tr_str(
                    &lang,
                    "正在向官方上游 API/CLI 实时验证凭据…",
                    "Verifying credentials live against upstream API/CLI…",
                )
                .to_string(),
            );
            if let Some(ref mut d) = h.marketplace.setup_draft {
                d.error = None;
                d.notice = None;
            }
            hcx.notify();
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let rt_bg = Arc::clone(&rt);
            let cid = connector_id.clone();
            let acct = account_label.clone().unwrap_or_default();
            let cred = credential.clone();
            let meta = metadata.clone();

            let verify_res = tokio_rt
                .spawn(async move {
                    let http = build_probe_http_client();
                    rt_bg
                        .marketplace()
                        .connect_native_connector_verified(
                            rt_bg.mcp(),
                            &http,
                            &cid,
                            &acct,
                            cred.as_deref(),
                            meta,
                        )
                        .await
                })
                .await;

            match verify_res {
                Ok(Ok(rec)) => {
                    // Sync verified native connector to cloud backend so Fabushi Bots immediately
                    // gain real live tools (slack_*, discord_*, telegram_*, github_*, x_*, whatsapp_*).
                    let mut server_conn_opt = None;
                    let mut server_installs = None;
                    if let Some(ref f) = fence {
                        let req = ConnectMarketplaceAppRequest {
                            app_id: rec.app_id.clone(),
                            slug: Some(connector_id.clone()),
                            kind: Some("connector".to_string()),
                            name: Some(rec.name.clone()),
                            account_label: rec.account_label.clone(),
                            credential: credential.clone(),
                            auth_values: BTreeMap::new(),
                            override_url: rec.endpoint.clone(),
                            transport: None,
                            command: None,
                            args: Vec::new(),
                            headers: Vec::new(),
                            metadata: rec.metadata.clone(),
                            // Local CLI vaults (op/bw/pass-cli) live on the user's machine;
                            // remote HTTPS connectors are also live-verified on the server.
                            verify_live: !is_vault_cli,
                            local_discovered_tools: to_proto_tools(&rec.discovered_tools),
                        };
                        let client = Arc::clone(&f.client);
                        let tok = f.token.clone();
                        let ep = f.client_epoch;
                        if let Ok(Ok(resp)) = tokio_rt
                            .spawn(async move {
                                client
                                    .connect_marketplace_app_with_token(&tok, ep, &req)
                                    .await
                            })
                            .await
                        {
                            server_conn_opt = Some(resp.connection);
                            server_installs = Some(resp.install_count);
                        }
                    }

                    let tool_n = rec.discovered_tools.len();
                    let label_suffix = rec
                        .account_label
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(|s| format!(" ({s})"))
                        .unwrap_or_default();
                    let msg = if lang.starts_with("zh") {
                        format!(
                            "已实时验证并连接 {}{} · {} 个可执行工具",
                            rec.name, label_suffix, tool_n
                        )
                    } else {
                        format!(
                            "Verified & connected {}{} · {} live tools",
                            rec.name, label_suffix, tool_n
                        )
                    };
                    let _ = weak.update(app, |shell, cx| {
                        shell.sync_marketplace_runtime_state(cx);
                        shell.refresh_active_bot_fusion_snapshot(cx);
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            if let Some(sc) = server_conn_opt {
                                h.marketplace
                                    .server_connections
                                    .insert(sc.app_id.clone(), sc);
                            }
                            if let Some(cnt) = server_installs {
                                h.marketplace
                                    .live_app_install_counts
                                    .insert(rec.app_id.clone(), cnt);
                            }
                            h.marketplace.error_banner = None;
                            h.marketplace.notice_banner = Some(msg.clone());
                            if let Some(ref mut d) = h.marketplace.setup_draft {
                                d.error = None;
                                d.notice = Some(msg.clone());
                            }
                            hcx.notify();
                        });
                        shell.show_non_chat_toast(msg, 3000, cx);
                        cx.notify();
                    });
                }
                Ok(Err(err)) => {
                    let err_msg = err.to_string();
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.notice_banner = None;
                            h.marketplace.error_banner = Some(err_msg.clone());
                            if let Some(ref mut d) = h.marketplace.setup_draft {
                                d.error = Some(err_msg);
                                d.notice = None;
                            }
                            hcx.notify();
                        });
                        cx.notify();
                    });
                }
                Err(join_err) => {
                    let err_msg = format!("Native verification task failed: {join_err}");
                    let _ = weak.update(app, |shell, cx| {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.connecting_app_id = None;
                            h.marketplace.error_banner = Some(err_msg);
                            hcx.notify();
                        });
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Re-probes an existing connected app live (via local MCP `tools/list` and/or server
    /// `/api/v1/marketplace/connections/{app_id}/probe`) to refresh discovered tools & health.
    pub fn handle_marketplace_probe_app(&mut self, app_id: String, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        let rt_opt = self.ensure_native_openbot_runtime();
        let fence = self.capture_client_fence();
        let tokio_rt = Arc::clone(self.client.runtime());
        let linked_server_id = self
            .content_hub
            .read(cx)
            .marketplace
            .linked_mcp_server_id(&app_id);

        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.probing_app_id = Some(app_id.clone());
            h.marketplace.error_banner = None;
            hcx.notify();
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            // Try server live probe first when connected to cloud backend (handles native connectors
            // & remote HTTP plugins with encrypted server secrets).
            if let Some(ref f) = fence {
                let client = Arc::clone(&f.client);
                let tok = f.token.clone();
                let ep = f.client_epoch;
                let aid = app_id.clone();
                if let Ok(Ok(resp)) = tokio_rt
                    .spawn(async move {
                        client
                            .probe_marketplace_connection_with_token(&tok, ep, &aid)
                            .await
                    })
                    .await
                {
                    let n = resp.connection.discovered_tools.len();
                    let status = resp.connection.status.clone();
                    let err_opt = resp.connection.last_error.clone();
                    if let Some(ref rt) = rt_opt {
                        let local_tools: Vec<McpDiscoveredTool> = resp
                            .connection
                            .discovered_tools
                            .iter()
                            .map(|t| McpDiscoveredTool {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                input_schema: t.input_schema.clone(),
                            })
                            .collect();
                        let _ = rt.marketplace().update_connection_probe_state(
                            &app_id,
                            &status,
                            local_tools,
                            err_opt.clone(),
                        );
                    }
                    let msg = if status == "connected" {
                        if lang.starts_with("zh") {
                            format!("实时探测正常：发现 {n} 个可执行工具")
                        } else {
                            format!("Live probe healthy: {n} executable tools")
                        }
                    } else {
                        err_opt.unwrap_or_else(|| format!("Probe status: {status}"))
                    };
                    let ok = status == "connected";
                    let _ = weak.update(app, |shell, cx| {
                        shell.sync_marketplace_runtime_state(cx);
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.probing_app_id = None;
                            h.marketplace
                                .server_connections
                                .insert(resp.connection.app_id.clone(), resp.connection);
                            if ok {
                                h.marketplace.error_banner = None;
                                h.marketplace.notice_banner = Some(msg.clone());
                            } else {
                                h.marketplace.notice_banner = None;
                                h.marketplace.error_banner = Some(msg.clone());
                            }
                            hcx.notify();
                        });
                        cx.notify();
                    });
                    return;
                }
            }

            // Fallback to local MCP probe (for local/custom STDIO or offline sessions).
            if let (Some(rt), Some(sid)) = (rt_opt, linked_server_id) {
                let rt_bg = Arc::clone(&rt);
                let probe_res = tokio_rt
                    .spawn(async move { rt_bg.mcp().probe_server(&sid).await })
                    .await;
                let _ = weak.update(app, |shell, cx| {
                    match probe_res {
                        Ok(Ok(outcome)) => {
                            let ok = outcome.status == "ready";
                            let _ = rt.marketplace().update_connection_probe_state(
                                &app_id,
                                if ok { "connected" } else { &outcome.status },
                                outcome.tool_defs.clone(),
                                outcome.error.clone(),
                            );
                            let msg = if ok {
                                if lang.starts_with("zh") {
                                    format!("本地 MCP 探测正常：发现 {} 个实时工具", outcome.tool_count)
                                } else {
                                    format!(
                                        "Local MCP probe healthy: {} live tools",
                                        outcome.tool_count
                                    )
                                }
                            } else {
                                outcome
                                    .error
                                    .unwrap_or_else(|| format!("Probe status: {}", outcome.status))
                            };
                            shell.sync_marketplace_runtime_state(cx);
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.probing_app_id = None;
                                if ok {
                                    h.marketplace.error_banner = None;
                                    h.marketplace.notice_banner = Some(msg);
                                } else {
                                    h.marketplace.notice_banner = None;
                                    h.marketplace.error_banner = Some(msg);
                                }
                                hcx.notify();
                            });
                        }
                        Ok(Err(e)) => {
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.probing_app_id = None;
                                h.marketplace.error_banner = Some(e.to_string());
                                hcx.notify();
                            });
                        }
                        Err(e) => {
                            shell.content_hub.update(cx, |h, hcx| {
                                h.marketplace.probing_app_id = None;
                                h.marketplace.error_banner = Some(e.to_string());
                                hcx.notify();
                            });
                        }
                    }
                    cx.notify();
                });
            } else {
                let _ = weak.update(app, |shell, cx| {
                    shell.content_hub.update(cx, |h, hcx| {
                        h.marketplace.probing_app_id = None;
                        hcx.notify();
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn handle_marketplace_disconnect_app(&mut self, app_id: String, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        let label = self
            .content_hub
            .read(cx)
            .marketplace
            .find_app(&app_id)
            .map(|a| a.name().to_string())
            .unwrap_or_else(|| app_id.clone());

        if let Some(rt) = self.ensure_native_openbot_runtime() {
            let _ = rt.marketplace().disconnect_app_via_mcp(rt.mcp(), &app_id);
        }

        // Also disconnect on cloud backend & remove its live handlers from AppState::mcp_registry.
        if let Some(fence) = self.capture_client_fence() {
            let tokio_rt = Arc::clone(self.client.runtime());
            let aid = app_id.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let client = Arc::clone(&fence.client);
                let tok = fence.token.clone();
                let ep = fence.client_epoch;
                let _ = tokio_rt
                    .spawn(async move {
                        client
                            .disconnect_marketplace_app_with_token(&tok, ep, &aid)
                            .await
                    })
                    .await;
            })
            .detach();
        }

        let msg = format!("{} {}", tr_str(&lang, "已断开连接：", "Disconnected "), label);
        self.sync_marketplace_runtime_state(cx);
        self.refresh_active_bot_fusion_snapshot(cx);
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.server_connections.remove(&app_id);
            h.marketplace.error_banner = None;
            h.marketplace.notice_banner = Some(msg.clone());
            hcx.notify();
        });
        self.show_non_chat_toast(msg, 2400, cx);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_marketplace_save_custom_mcp(
        &mut self,
        edit_server_id: Option<String>,
        name: String,
        transport: McpTransport,
        command: String,
        args: Vec<String>,
        env: Vec<McpKeyValue>,
        working_dir: String,
        url: String,
        headers: Vec<McpKeyValue>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let candidate = McpServerConfig {
            id: edit_server_id.clone().unwrap_or_default(),
            name: name.trim().to_string(),
            transport,
            enabled: true,
            command: command.trim().to_string(),
            args: args.clone(),
            env: env.clone(),
            env_passthrough: Vec::new(),
            working_directory: working_dir.trim().to_string(),
            url: url.trim().to_string(),
            headers: headers.clone(),
        };
        let errs = mcp_config_errors(&candidate);
        if let Some(first) = errs.first_error() {
            let msg = first.to_string();
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.custom_form.error = Some(msg.clone());
                h.marketplace.error_banner = Some(msg);
                hcx.notify();
            });
            cx.notify();
            return;
        }
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        match rt.mcp().register_server(RegisterMcpServerInput {
            id: edit_server_id,
            name: candidate.name.clone(),
            transport,
            enabled: Some(true),
            command: (!candidate.command.is_empty()).then_some(candidate.command),
            args,
            env,
            env_passthrough: Vec::new(),
            working_directory: (!candidate.working_directory.is_empty())
                .then_some(candidate.working_directory),
            url: (!candidate.url.is_empty()).then_some(candidate.url),
            headers,
        }) {
            Ok(saved) => {
                let msg = format!(
                    "{} {}",
                    tr_str(&lang, "已保存自定义 MCP 服务器：", "Saved custom MCP server: "),
                    saved.name
                );
                self.sync_marketplace_runtime_state(cx);
                self.refresh_active_bot_fusion_snapshot(cx);
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.custom_form.error = None;
                    h.marketplace.error_banner = None;
                    h.marketplace.notice_banner = Some(msg.clone());
                    h.marketplace.stack = vec![
                        MarketplaceRoute::Browse,
                        MarketplaceRoute::AppDetail {
                            app_id: format!("custom:{}", saved.id),
                        },
                    ];
                    hcx.notify();
                });
                self.show_non_chat_toast(msg, 2600, cx);
                // Automatically run a live MCP probe on the newly saved custom server!
                self.handle_marketplace_probe_app(format!("custom:{}", saved.id), cx);
            }
            Err(err) => {
                let err_msg = err.to_string();
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.custom_form.error = Some(err_msg.clone());
                    h.marketplace.error_banner = Some(err_msg);
                    hcx.notify();
                });
            }
        }
        cx.notify();
    }

    /// Publishes a real custom STDIO or Streamable HTTP connector directly to `gpui_telegram_server`
    /// (`POST /api/v1/marketplace/apps`) so it is stored in SQLite and immediately visible in the
    /// Marketplace catalog for all users/clients.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_marketplace_publish_custom_app(
        &mut self,
        name: String,
        tagline: String,
        category: String,
        transport: McpTransport,
        command: String,
        args: Vec<String>,
        env: Vec<McpKeyValue>,
        url: String,
        headers: Vec<McpKeyValue>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let candidate = McpServerConfig {
            id: String::new(),
            name: name.trim().to_string(),
            transport,
            enabled: true,
            command: command.trim().to_string(),
            args: args.clone(),
            env: env.clone(),
            env_passthrough: Vec::new(),
            working_directory: String::new(),
            url: url.trim().to_string(),
            headers: headers.clone(),
        };
        let errs = mcp_config_errors(&candidate);
        if let Some(first) = errs.first_error() {
            let msg = first.to_string();
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.custom_form.error = Some(msg.clone());
                h.marketplace.error_banner = Some(msg);
                hcx.notify();
            });
            return;
        }
        let Some(fence) = self.capture_client_fence() else {
            let msg = tr_str(
                &lang,
                "需要登录云端账号才能将连接器上架到市场。",
                "Sign in to publish connectors to the live Marketplace.",
            )
            .to_string();
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.custom_form.error = Some(msg.clone());
                h.marketplace.error_banner = Some(msg);
                hcx.notify();
            });
            return;
        };

        let tokio_rt = Arc::clone(self.client.runtime());
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.custom_form.error = None;
            h.marketplace.custom_form.test_result = Some((
                true,
                tr_str(
                    &lang,
                    "正在实时校验并上架连接器到市场数据库…",
                    "Verifying & publishing connector to live Marketplace…",
                )
                .to_string(),
            ));
            hcx.notify();
        });

        let is_remote_http =
            transport == McpTransport::Http && !is_local_loopback_url(&candidate.url);
        let req = PublishMarketplaceAppRequest {
            name: candidate.name.clone(),
            slug: None,
            tagline: if tagline.trim().is_empty() {
                format!("{} MCP Connector", candidate.name)
            } else {
                tagline.trim().to_string()
            },
            zh_tagline: (!tagline.trim().is_empty()).then(|| tagline.trim().to_string()),
            description: Some(if tagline.trim().is_empty() {
                format!("Live MCP connector for {}.", candidate.name)
            } else {
                tagline.trim().to_string()
            }),
            zh_description: (!tagline.trim().is_empty()).then(|| tagline.trim().to_string()),
            category: Some(if category.trim().is_empty() {
                "other".to_string()
            } else {
                category
            }),
            brand_hex: Some("#0A84FF".to_string()),
            website_url: None,
            transport: match transport {
                McpTransport::Http => "http".to_string(),
                McpTransport::Stdio => "stdio".to_string(),
            },
            url: (transport == McpTransport::Http).then_some(candidate.url.clone()),
            command: (transport == McpTransport::Stdio).then_some(candidate.command.clone()),
            args,
            env: env
                .into_iter()
                .map(|kv| MarketplaceEnvPairDto {
                    key: kv.key,
                    value: kv.value,
                })
                .collect(),
            auth_kind: Some(if transport == McpTransport::Http && !headers.is_empty() {
                "key".to_string()
            } else if is_local_loopback_url(&candidate.url) {
                "local".to_string()
            } else {
                "open".to_string()
            }),
            auth_fields: Vec::new(),
            verify_before_publish: is_remote_http,
        };

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let client = Arc::clone(&fence.client);
            let tok = fence.token.clone();
            let ep = fence.client_epoch;
            let res = tokio_rt
                .spawn(async move { client.publish_marketplace_app_with_token(&tok, ep, &req).await })
                .await;

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                match res {
                    Ok(Ok(published)) => {
                        let app_id = published.app_id.clone();
                        let msg = if lang.starts_with("zh") {
                            format!("已真实上架连接器「{}」到市场！", published.name)
                        } else {
                            format!("Published connector \"{}\" to live Marketplace!", published.name)
                        };
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace
                                .published_custom_apps
                                .retain(|a| a.app_id != published.app_id);
                            h.marketplace.published_custom_apps.push(published);
                            h.marketplace.custom_form.error = None;
                            h.marketplace.custom_form.test_result = None;
                            h.marketplace.error_banner = None;
                            h.marketplace.notice_banner = Some(msg.clone());
                            h.marketplace.stack = vec![
                                MarketplaceRoute::Browse,
                                MarketplaceRoute::AppDetail { app_id },
                            ];
                            hcx.notify();
                        });
                        shell.show_non_chat_toast(msg, 3000, cx);
                        shell.refresh_marketplace_from_server(cx);
                    }
                    Ok(Err(err)) => {
                        let err_msg = err.to_string();
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.custom_form.error = Some(err_msg.clone());
                            h.marketplace.custom_form.test_result = Some((false, err_msg.clone()));
                            h.marketplace.error_banner = Some(err_msg);
                            hcx.notify();
                        });
                    }
                    Err(join_err) => {
                        let err_msg = format!("Publish task failed: {join_err}");
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.custom_form.error = Some(err_msg.clone());
                            h.marketplace.error_banner = Some(err_msg);
                            hcx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a real live Streamable HTTP or STDIO MCP handshake (`initialize` + `tools/list`) to test
    /// a custom server before saving or publishing, displaying exact discovered tool counts & names.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_marketplace_test_custom_mcp(
        &mut self,
        name: String,
        transport: McpTransport,
        command: String,
        args: Vec<String>,
        env: Vec<McpKeyValue>,
        working_dir: String,
        url: String,
        headers: Vec<McpKeyValue>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let candidate = McpServerConfig {
            id: "probe-temp".to_string(),
            name: if name.trim().is_empty() {
                "Custom MCP".to_string()
            } else {
                name.trim().to_string()
            },
            transport,
            enabled: true,
            command: command.trim().to_string(),
            args,
            env,
            env_passthrough: Vec::new(),
            working_directory: working_dir.trim().to_string(),
            url: url.trim().to_string(),
            headers: headers.clone(),
        };
        let errs = mcp_config_errors(&candidate);
        if let Some(first) = errs.first_error() {
            let msg = first.to_string();
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.custom_form.test_result = Some((false, msg));
                hcx.notify();
            });
            cx.notify();
            return;
        }

        let tokio_rt = Arc::clone(self.client.runtime());
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.custom_form.test_result = Some((
                true,
                tr_str(
                    &lang,
                    "正在执行真实 MCP initialize + tools/list 握手测试…",
                    "Running live MCP initialize + tools/list handshake…",
                )
                .to_string(),
            ));
            hcx.notify();
        });

        match transport {
            McpTransport::Stdio => {
                let rt_opt = self.ensure_native_openbot_runtime();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let probe_out: Result<String, String> = if let Some(rt) = rt_opt {
                        let cand = candidate.clone();
                        let res = tokio_rt
                            .spawn(async move {
                                // Register ephemeral probe server, run real STDIO JSON-RPC initialize+tools/list, then remove!
                                let tmp_id = format!(
                                    "probe-stdio-{}",
                                    uuid::Uuid::new_v4().simple()
                                );
                                let saved = rt
                                    .mcp()
                                    .register_server(RegisterMcpServerInput {
                                        id: Some(tmp_id.clone()),
                                        name: cand.name.clone(),
                                        transport: McpTransport::Stdio,
                                        enabled: Some(true),
                                        command: Some(cand.command.clone()),
                                        args: cand.args.clone(),
                                        env: cand.env.clone(),
                                        env_passthrough: Vec::new(),
                                        working_directory: (!cand.working_directory.is_empty())
                                            .then_some(cand.working_directory.clone()),
                                        url: None,
                                        headers: Vec::new(),
                                    })
                                    .map_err(|e| e.to_string())?;
                                let outcome = rt.mcp().probe_server(&saved.id).await;
                                let _ = rt.mcp().remove_server(&saved.id);
                                outcome.map_err(|e| e.to_string())
                            })
                            .await;
                        match res {
                            Ok(Ok(outcome)) if outcome.status == "ready" => {
                                let preview = outcome
                                    .tools
                                    .iter()
                                    .take(5)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                if lang.starts_with("zh") {
                                    Ok(format!(
                                        "STDIO MCP 握手成功：发现 {} 个工具{}",
                                        outcome.tool_count,
                                        if preview.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" ({preview})")
                                        }
                                    ))
                                } else {
                                    Ok(format!(
                                        "STDIO MCP handshake OK: {} tools{}",
                                        outcome.tool_count,
                                        if preview.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" ({preview})")
                                        }
                                    ))
                                }
                            }
                            Ok(Ok(outcome)) => Err(outcome
                                .error
                                .unwrap_or_else(|| format!("STDIO probe status: {}", outcome.status))),
                            Ok(Err(e)) => Err(e),
                            Err(e) => Err(e.to_string()),
                        }
                    } else {
                        Err("OpenBot local runtime unavailable.".to_string())
                    };
                    let _ = weak.update(app, |shell, cx| {
                        let res = match probe_out {
                            Ok(msg) => (true, msg),
                            Err(err) => (false, err),
                        };
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.custom_form.test_result = Some(res);
                            hcx.notify();
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
            McpTransport::Http => {
                let target_url = candidate.url.clone();
                let srv_name = candidate.name.clone();
                let probe_headers = headers;
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = tokio_rt
                        .spawn(async move {
                            let http = build_probe_http_client();
                            probe_http_mcp_endpoint(
                                &http,
                                "probe-http",
                                &srv_name,
                                &target_url,
                                &probe_headers,
                            )
                            .await
                        })
                        .await;
                    let _ = weak.update(app, |shell, cx| {
                        let ui_res = match res {
                            Ok(outcome) if outcome.status == "ready" => {
                                let preview = outcome
                                    .tools
                                    .iter()
                                    .take(5)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                let msg = if lang.starts_with("zh") {
                                    format!(
                                        "Streamable HTTP MCP 握手成功：发现 {} 个工具{}",
                                        outcome.tool_count,
                                        if preview.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" ({preview})")
                                        }
                                    )
                                } else {
                                    format!(
                                        "Streamable HTTP MCP OK: {} tools{}",
                                        outcome.tool_count,
                                        if preview.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" ({preview})")
                                        }
                                    )
                                };
                                (true, msg)
                            }
                            Ok(outcome) if outcome.status == "needs_sign_in" => (
                                false,
                                tr_str(
                                    &lang,
                                    "端点返回 HTTP 401：需要配置 Authorization 请求头或 OAuth 授权。",
                                    "Endpoint returned HTTP 401: add an Authorization header or sign in via OAuth.",
                                )
                                .to_string(),
                            ),
                            Ok(outcome) => (
                                false,
                                outcome
                                    .error
                                    .unwrap_or_else(|| format!("Probe failed: {}", outcome.status)),
                            ),
                            Err(e) => (false, e.to_string()),
                        };
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace.custom_form.test_result = Some(ui_res);
                            hcx.notify();
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    pub fn handle_marketplace_remove_custom_mcp(
        &mut self,
        server_id: String,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        if let Err(err) = rt.mcp().remove_server(&server_id) {
            let msg = err.to_string();
            self.content_hub.update(cx, |h, hcx| {
                h.marketplace.error_banner = Some(msg);
                hcx.notify();
            });
            return;
        }
        if let Some(fence) = self.capture_client_fence() {
            let tokio_rt = Arc::clone(self.client.runtime());
            let sid = server_id.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let client = Arc::clone(&fence.client);
                let tok = fence.token.clone();
                let ep = fence.client_epoch;
                let _ = tokio_rt
                    .spawn(async move { client.delete_mcp_server_with_token(&tok, ep, &sid).await })
                    .await;
            })
            .detach();
        }
        let msg = tr_str(&lang, "已移除自定义 MCP 服务器", "Removed custom MCP server").to_string();
        self.sync_marketplace_runtime_state(cx);
        self.refresh_active_bot_fusion_snapshot(cx);
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.notice_banner = Some(msg.clone());
            hcx.notify();
        });
        self.show_non_chat_toast(msg, 2400, cx);
        cx.notify();
    }

    pub fn handle_marketplace_toggle_server_agent_access(
        &mut self,
        server_id: String,
        agent_id: String,
        allowed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let all_ids: Vec<String> = rt
            .agents()
            .list_agents()
            .into_iter()
            .map(|a| a.id)
            .collect();
        let current_map = rt.mcp().list_server_access();
        let next_allowed: Option<Vec<String>> = match current_map.get(&server_id).cloned().flatten()
        {
            None => {
                if allowed {
                    None
                } else {
                    Some(all_ids.into_iter().filter(|id| id != &agent_id).collect())
                }
            }
            Some(mut list) => {
                if allowed {
                    if !list.iter().any(|id| id == &agent_id) {
                        list.push(agent_id);
                    }
                } else {
                    list.retain(|id| id != &agent_id);
                }
                if !all_ids.is_empty() && all_ids.iter().all(|id| list.iter().any(|x| x == id)) {
                    None
                } else {
                    Some(list)
                }
            }
        };
        let _ = rt.mcp().set_server_access(&server_id, next_allowed);
        self.sync_marketplace_runtime_state(cx);
        self.refresh_active_bot_fusion_snapshot(cx);
        cx.notify();
    }

    pub fn handle_marketplace_add_agent(&mut self, slug: String, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        let catalog = openbot_marketplace_catalog();
        let Some(agent_tpl) = catalog.agents.iter().find(|a| a.slug == slug) else {
            return;
        };
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };

        let foreign_title = format!("marketplace:{}", agent_tpl.slug);
        let existing_id = rt
            .agents()
            .list_agents()
            .into_iter()
            .find(|a| a.title == foreign_title)
            .map(|a| a.id);

        let hue = nearest_avatar_hue(agent_tpl.avatar_hue);
        let local_agent_id = match existing_id {
            Some(id) => id,
            None => {
                let req = CreateAgentRequest {
                    name: agent_tpl.name.clone(),
                    title: foreign_title,
                    description: agent_tpl.description.clone(),
                    allow_automation: Some(true),
                    computer_use: Some(true),
                    avatar_seed: Some(agent_tpl.slug.clone()),
                    avatar_hue: Some(hue),
                    avatar_color: None,
                    ..Default::default()
                };
                match rt.agents().create_agent_full(req) {
                    Ok(created) => created.id,
                    Err(err) => {
                        let msg = err.to_string();
                        self.content_hub.update(cx, |h, hcx| {
                            h.marketplace.error_banner = Some(msg);
                            hcx.notify();
                        });
                        return;
                    }
                }
            }
        };

        for sk_slug in &agent_tpl.skills {
            let (sk_name, sk_desc, sk_md) = catalog
                .skills
                .iter()
                .find(|s| s.slug == *sk_slug)
                .map(|s| (s.name.clone(), s.description.clone(), s.skill_md.clone()))
                .unwrap_or_else(|| {
                    (
                        sk_slug.replace('-', " "),
                        format!("Bundled skill for {}", agent_tpl.name),
                        format!("# {sk_slug}\nBundled skill for {}.", agent_tpl.name),
                    )
                });
            let _ = rt.skills().save_skill(SaveSkillInput {
                agent_id: local_agent_id.clone(),
                slug: sk_slug.clone(),
                name: sk_name,
                description: sk_desc,
                instructions: sk_md,
                triggers: vec![format!("${sk_slug}")],
                tools: Vec::new(),
                origin: Some("marketplace".to_string()),
                enabled: Some(true),
            });
        }

        for routine in &agent_tpl.routines {
            let _ = rt.routines().save_routine(RoutineSaveRequest {
                owner_id: local_agent_id.clone(),
                routine_id: None,
                is_channel: false,
                name: routine.name.clone(),
                instruction: routine.instruction.clone(),
                timezone: "UTC".to_string(),
                active: routine.active,
                schedule: Some(parse_marketplace_routine_schedule(routine)),
                webhook_event_type: None,
                webhook_filters: Vec::new(),
                webhook_secret: None,
                limit_policy: RoutineLimitPolicy::Wait,
                missed_policy: RoutineMissedPolicy::Skip,
                run_in: RoutineRunPlace::Chat,
            });
        }

        let _ = rt
            .marketplace()
            .install_item(MarketplaceTab::Agents, &agent_tpl.slug);

        let agent_name = agent_tpl.name.clone();
        let agent_slug = agent_tpl.slug.clone();
        let bundled_skills = agent_tpl.skills.clone();
        let toast_msg = format!(
            "{} {}",
            tr_str(&lang, "已添加助手：", "Added agent: "),
            agent_name
        );

        self.sync_marketplace_runtime_state(cx);
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.added_agent_slugs.insert(agent_slug.clone());
            h.marketplace.error_banner = None;
            h.marketplace.notice_banner = Some(toast_msg.clone());
            hcx.notify();
        });
        self.show_non_chat_toast(toast_msg, 2600, cx);

        if let Some(fence) = self.capture_client_fence() {
            let tokio_rt = Arc::clone(self.client.runtime());
            let slug_for_count = agent_slug.clone();
            let safe_slug = sanitize_slug_id(&agent_tpl.slug);
            let already_has_bot = self
                .miniapps
                .fabushi_bots
                .iter()
                .any(|b| b.agent_id == format!("ob_{safe_slug}") || b.display_name == agent_tpl.name);
            let bot_req = (!already_has_bot).then(|| CreateFabushiBotRequest {
                display_name: agent_tpl.name.clone(),
                username: None,
                agent_id: Some(format!("ob_{safe_slug}")),
                description: agent_tpl.description.clone(),
                title: agent_tpl.title.clone(),
                avatar_shape: None,
                avatar_color: None,
                ..Default::default()
            });
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let client_cnt = Arc::clone(&fence.client);
                let tok_cnt = fence.token.clone();
                let ep_cnt = fence.client_epoch;
                let count_res = tokio_rt
                    .spawn(async move {
                        client_cnt
                            .record_marketplace_agent_install_with_token(
                                &tok_cnt,
                                ep_cnt,
                                &slug_for_count,
                            )
                            .await
                    })
                    .await;

                let created_bot = if let Some(req) = bot_req {
                    let client_b = Arc::clone(&fence.client);
                    let tok_b = fence.token.clone();
                    let ep_b = fence.client_epoch;
                    tokio_rt
                        .spawn(async move {
                            client_b
                                .create_fabushi_bot_with_token(&tok_b, ep_b, &req)
                                .await
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                } else {
                    None
                };

                let _ = weak.update(app, |shell, cx| {
                    if !fence.is_valid(shell) {
                        return;
                    }
                    if let Ok(Ok(inst)) = count_res {
                        shell.content_hub.update(cx, |h, hcx| {
                            h.marketplace
                                .live_agent_install_counts
                                .insert(inst.slug, inst.install_count);
                            hcx.notify();
                        });
                    }
                    if let Some(created) = created_bot {
                        if let Some(slot) = shell
                            .miniapps
                            .fabushi_bots
                            .iter_mut()
                            .find(|b| b.bot_user_id == created.bot.bot_user_id)
                        {
                            *slot = created.bot.clone();
                        } else {
                            shell.miniapps.fabushi_bots.push(created.bot.clone());
                        }
                        shell.provision_openbot_agent_for_fabushi(&created.bot, &bundled_skills);
                        shell.sync_fabushi_bots_to_views(cx);
                        shell.refresh_chats(cx);
                        shell.sync_marketplace_runtime_state(cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub fn handle_marketplace_open_agent_chat(&mut self, slug: String, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        let safe_slug = sanitize_slug_id(&slug);
        let catalog_name = openbot_marketplace_catalog()
            .agents
            .iter()
            .find(|a| a.slug == slug)
            .map(|a| a.name.clone());

        let matched_bot = self.miniapps.fabushi_bots.iter().find(|b| {
            b.agent_id == format!("ob_{safe_slug}")
                || b.agent_id == slug
                || catalog_name
                    .as_deref()
                    .is_some_and(|n| b.display_name.eq_ignore_ascii_case(n))
        });

        let target_chat = matched_bot.and_then(|b| {
            b.companion_chat_id
                .filter(|&id| id > 0)
                .map(|id| (id as u64, b.display_name.clone()))
        });

        self.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        if let Some((cid, title)) = target_chat {
            self.open_or_select_chat(cid, Some(title), true, cx);
        } else {
            let name = catalog_name.unwrap_or(slug);
            self.show_non_chat_toast(
                format!(
                    "{} {}",
                    tr_str(&lang, "已切换到消息，可开始与助手对话：", "Switched to Chats for "),
                    name
                ),
                2400,
                cx,
            );
        }
    }

    pub fn handle_marketplace_toggle_skill_for_agent(
        &mut self,
        slug: String,
        agent_id: String,
        install: bool,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let catalog = openbot_marketplace_catalog();
        let skill_opt = catalog.skills.iter().find(|s| s.slug == slug);

        let outcome: Result<String, String> = if install {
            let (name, desc, md) = skill_opt
                .map(|s| (s.name.clone(), s.description.clone(), s.skill_md.clone()))
                .unwrap_or_else(|| {
                    (
                        slug.replace('-', " "),
                        format!("Marketplace skill {slug}"),
                        format!("# {slug}"),
                    )
                });
            rt.skills()
                .save_skill(SaveSkillInput {
                    agent_id: agent_id.clone(),
                    slug: slug.clone(),
                    name: name.clone(),
                    description: desc,
                    instructions: md,
                    triggers: vec![format!("${slug}")],
                    tools: Vec::new(),
                    origin: Some("marketplace".to_string()),
                    enabled: Some(true),
                })
                .map(|_| {
                    let _ = rt.marketplace().install_item(MarketplaceTab::Skills, &slug);
                    format!("{} {}", tr_str(&lang, "已安装技能：", "Installed skill: "), name)
                })
                .map_err(|e| e.to_string())
        } else {
            let found_id = rt
                .skills()
                .list_skills(&agent_id)
                .unwrap_or_default()
                .into_iter()
                .find(|s| s.slug == slug)
                .map(|s| s.skill_id);
            if let Some(sid) = found_id {
                rt.skills()
                    .uninstall_skill(&agent_id, &sid)
                    .map(|_| {
                        format!(
                            "{} {}",
                            tr_str(&lang, "已移除技能：", "Removed skill: "),
                            skill_opt.map(|s| s.name.as_str()).unwrap_or(&slug)
                        )
                    })
                    .map_err(|e| e.to_string())
            } else {
                Ok(tr_str(&lang, "技能已移除", "Skill removed").to_string())
            }
        };

        match outcome {
            Ok(msg) => {
                self.sync_marketplace_runtime_state(cx);
                self.refresh_active_bot_fusion_snapshot(cx);
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = None;
                    h.marketplace.notice_banner = Some(msg.clone());
                    hcx.notify();
                });
                self.show_non_chat_toast(msg, 2400, cx);
                if install {
                    if let Some(fence) = self.capture_client_fence() {
                        let tokio_rt = Arc::clone(self.client.runtime());
                        let slug_c = slug.clone();
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let client = Arc::clone(&fence.client);
                            let tok = fence.token.clone();
                            let ep = fence.client_epoch;
                            if let Ok(Ok(inst)) = tokio_rt
                                .spawn(async move {
                                    client
                                        .record_marketplace_skill_install_with_token(
                                            &tok, ep, &slug_c,
                                        )
                                        .await
                                })
                                .await
                            {
                                let _ = weak.update(app, |shell, cx| {
                                    if !fence.is_valid(shell) {
                                        return;
                                    }
                                    shell.content_hub.update(cx, |h, hcx| {
                                        h.marketplace
                                            .live_skill_install_counts
                                            .insert(inst.slug, inst.install_count);
                                        hcx.notify();
                                    });
                                    cx.notify();
                                });
                            }
                        })
                        .detach();
                    }
                }
            }
            Err(err) => {
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = Some(err);
                    hcx.notify();
                });
            }
        }
        cx.notify();
    }

    pub fn handle_marketplace_run_prompt(
        &mut self,
        app_name: String,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        self.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        let focus_composer = self.can_focus_chat_composer(cx);
        self.composer.update(cx, |c, c_cx| {
            c.set_text(prompt);
            c.sync_autocomplete();
            c.is_input_focused = focus_composer;
            c_cx.notify();
        });
        self.show_non_chat_toast(
            format!(
                "{} {}",
                app_name,
                tr_str(&lang, "示例提示词已填入输入框", "prompt loaded into composer")
            ),
            2400,
            cx,
        );
        cx.notify();
    }

    pub fn handle_marketplace_copy_link(&mut self, url: String, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        let msg = tr_str(&lang, "已复制市场链接", "Marketplace link copied").to_string();
        self.content_hub.update(cx, |h, hcx| {
            h.marketplace.notice_banner = Some(msg.clone());
            hcx.notify();
        });
        self.show_non_chat_toast(msg, 2000, cx);
    }
}
