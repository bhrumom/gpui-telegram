//! `AppShell` coordinator for the unified OpenBot 1:1 Marketplace (`市场`):
//! - Live persistence of native connectors (`Slack`, `Discord`, `Telegram`, `WhatsApp`,
//!   `1Password`, `Bitwarden`, `Proton Pass`, `X`, `GitHub`), 257 OpenBot MCP plugins,
//!   and custom STDIO / Streamable HTTP MCP servers inside `OpenBotHostRuntime::mcp()` &
//!   `openbot-connectors-v1.json`.
//! - Live agent provisioning (local OpenBot agent + bundled `SKILL.md` skills + routines +
//!   managed Fabushi Bot creation when signed in).
//! - Live per-agent skill installation/uninstallation and per-server agent access control,
//!   synchronized automatically with Bot Inspector (`BotFusionSnapshot`) and `$` autocomplete.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use gpui::{AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_proto::miniapp_contracts::CreateFabushiBotRequest;
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::routines::{
    RoutineLimitPolicy, RoutineMissedPolicy, RoutineRunPlace, RoutineSchedule,
};
use openbot_runtime::agent_lifecycle::CreateAgentRequest;
use openbot_runtime::platform::marketplace::{
    openbot_marketplace_catalog, MarketplaceTab, OpenBotAgentRoutineSpec,
};
use openbot_runtime::platform::routines::RoutineSaveRequest;
use openbot_runtime::skill_mcp_service::{RegisterMcpServerInput, SaveSkillInput};
use openbot_storage::{mcp_config_errors, McpKeyValue, McpServerConfig, McpTransport};

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

impl AppShell {
    /// Opens the unified OpenBot Marketplace (`市场`) directly on `tab` (`应用`, `助手`, `技能`,
    /// `小程序应用`, `图书`, or `游戏`) and synchronizes live connector/agent/skill runtime state.
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
            MarketplaceNavTab::Apps | MarketplaceNavTab::Agents | MarketplaceNavTab::Skills => {}
        }
        cx.notify();
    }

    /// Synchronizes live `OpenBotHostRuntime` connectors, MCP servers, per-server agent access,
    /// installed agent slugs, and installed skills into `ContentHubWorkspace::marketplace`.
    pub fn sync_marketplace_runtime_state(&mut self, cx: &mut Context<Self>) {
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        // Ensure any cached Fabushi Bots also exist in OpenBotHostRuntime so users can install
        // skills and toggle connector access for all of their chat bots immediately.
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

        // Also match any Fabushi bot created with an `ob_<slug>` agent_id.
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
        match rt.marketplace().connect_plugin_via_mcp(
            rt.mcp(),
            &slug,
            &auth_values,
            override_url.as_deref(),
        ) {
            Ok((rec, _srv)) => {
                let msg = format!("{} {}", tr_str(&lang, "已连接", "Connected"), rec.name);
                self.sync_marketplace_runtime_state(cx);
                self.refresh_active_bot_fusion_snapshot(cx);
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = None;
                    h.marketplace.notice_banner = Some(msg.clone());
                    if let Some(ref mut d) = h.marketplace.setup_draft {
                        d.error = None;
                        d.notice = Some(msg.clone());
                    }
                    hcx.notify();
                });
                self.show_non_chat_toast(msg, 2600, cx);
            }
            Err(err) => {
                let err_msg = err.to_string();
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = Some(err_msg.clone());
                    if let Some(ref mut d) = h.marketplace.setup_draft {
                        d.error = Some(err_msg.clone());
                    }
                    hcx.notify();
                });
            }
        }
        cx.notify();
    }

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
        match rt.marketplace().connect_native_connector(
            rt.mcp(),
            &connector_id,
            account_label.as_deref().unwrap_or(""),
            credential.as_deref(),
            metadata,
        ) {
            Ok(rec) => {
                let msg = format!("{} {}", tr_str(&lang, "已连接", "Connected"), rec.name);
                self.sync_marketplace_runtime_state(cx);
                self.refresh_active_bot_fusion_snapshot(cx);
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = None;
                    h.marketplace.notice_banner = Some(msg.clone());
                    if let Some(ref mut d) = h.marketplace.setup_draft {
                        d.error = None;
                        d.notice = Some(msg.clone());
                    }
                    hcx.notify();
                });
                self.show_non_chat_toast(msg, 2600, cx);
            }
            Err(err) => {
                let err_msg = err.to_string();
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = Some(err_msg.clone());
                    if let Some(ref mut d) = h.marketplace.setup_draft {
                        d.error = Some(err_msg.clone());
                    }
                    hcx.notify();
                });
            }
        }
        cx.notify();
    }

    pub fn handle_marketplace_disconnect_app(&mut self, app_id: String, cx: &mut Context<Self>) {
        let lang = self.active_lang_id().to_string();
        let Some(rt) = self.ensure_native_openbot_runtime() else {
            return;
        };
        let label = rt
            .marketplace()
            .get_connected_app(&app_id)
            .map(|r| r.name)
            .unwrap_or_else(|| app_id.clone());
        match rt.marketplace().disconnect_app_via_mcp(rt.mcp(), &app_id) {
            Ok(_) => {
                let msg = format!("{} {}", tr_str(&lang, "已断开连接：", "Disconnected "), label);
                self.sync_marketplace_runtime_state(cx);
                self.refresh_active_bot_fusion_snapshot(cx);
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = None;
                    h.marketplace.notice_banner = Some(msg.clone());
                    hcx.notify();
                });
                self.show_non_chat_toast(msg, 2400, cx);
            }
            Err(err) => {
                let err_msg = err.to_string();
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.error_banner = Some(err_msg);
                    hcx.notify();
                });
            }
        }
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
        match transport {
            McpTransport::Stdio => {
                let cmd_name = candidate.command.clone();
                let found = std::path::Path::new(&cmd_name).exists()
                    || std::env::var_os("PATH").is_some_and(|paths| {
                        std::env::split_paths(&paths).any(|dir| {
                            let full = dir.join(&cmd_name);
                            full.is_file() || dir.join(format!("{cmd_name}.exe")).is_file()
                        })
                    });
                let result = if found {
                    (
                        true,
                        format!(
                            "{} `{cmd_name}`",
                            tr_str(&lang, "已检测到可执行命令：", "Executable found on PATH:")
                        ),
                    )
                } else {
                    (
                        false,
                        format!(
                            "{} `{cmd_name}`",
                            tr_str(
                                &lang,
                                "未在系统 PATH 中找到命令：",
                                "Command not found in PATH:",
                            )
                        ),
                    )
                };
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.custom_form.test_result = Some(result);
                    hcx.notify();
                });
                cx.notify();
            }
            McpTransport::Http => {
                let target_url = candidate.url.clone();
                let probe_headers = headers;
                self.content_hub.update(cx, |h, hcx| {
                    h.marketplace.custom_form.test_result = Some((
                        true,
                        tr_str(&lang, "正在探测端点…", "Probing endpoint…").to_string(),
                    ));
                    hcx.notify();
                });
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let probe_out = (|| async {
                        let client = reqwest::Client::builder()
                            .timeout(Duration::from_secs(5))
                            .build()
                            .map_err(|e| e.to_string())?;
                        let mut req = client.post(&target_url).json(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "initialize",
                            "params": {
                                "protocolVersion": "2024-11-05",
                                "capabilities": {},
                                "clientInfo": { "name": "fabushi-openbot", "version": env!("CARGO_PKG_VERSION") }
                            }
                        }));
                        for kv in &probe_headers {
                            req = req.header(&kv.key, &kv.value);
                        }
                        let resp = req.send().await.map_err(|e| e.to_string())?;
                        let status = resp.status();
                        if status.is_success() || status.as_u16() == 401 || status.as_u16() == 403 {
                            Ok(format!(
                                "{} (HTTP {})",
                                tr_str(&lang, "端点可达", "Endpoint reachable"),
                                status.as_u16()
                            ))
                        } else {
                            Err(format!("HTTP {}", status.as_u16()))
                        }
                    })()
                    .await;
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

        // Install every bundled skill (using full SKILL.md from the catalog when available).
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

        // Install every bundled routine for the added agent.
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

        // Also provision a live Fabushi Bot in Chats when connected with an authenticated fence.
        if let Some(fence) = self.capture_client_fence() {
            let safe_slug = sanitize_slug_id(&agent_tpl.slug);
            let already_has_bot = self
                .miniapps
                .fabushi_bots
                .iter()
                .any(|b| b.agent_id == format!("ob_{safe_slug}") || b.display_name == agent_tpl.name);
            if !already_has_bot {
                let req = CreateFabushiBotRequest {
                    display_name: agent_tpl.name.clone(),
                    username: None,
                    agent_id: Some(format!("ob_{safe_slug}")),
                    description: agent_tpl.description.clone(),
                    title: agent_tpl.title.clone(),
                    avatar_shape: None,
                    avatar_color: None,
                    ..Default::default()
                };
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    if let Ok(created) = fence
                        .client
                        .create_fabushi_bot_with_token(&fence.token, fence.client_epoch, &req)
                        .await
                    {
                        let _ = weak.update(app, |shell, cx| {
                            if !fence.is_valid(shell) {
                                return;
                            }
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
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
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
