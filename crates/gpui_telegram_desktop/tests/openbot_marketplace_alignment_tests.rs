//! End-to-end verification tests for 1:1 OpenBot Marketplace alignment:
//! 1. Removal of `MCP 工具` (`McpTools`) and `定时任务` (`Schedule`) entry points from the left rail,
//!    More menu, and persisted pin loading.
//! 2. Relocation of `图书` (`Books`), `游戏` (`Games`), and `小程序应用` (`AppStore` / `MiniApps`)
//!    into the Marketplace (`市场`) strictly separated below `技能` (`Skills`).
//! 3. Full OpenBot connector/plugin/agent/skill catalog parity (9 native connectors + 257 plugins =
//!    266 connector apps with exact upstream category counts, 29 agents, 40 skills).
//! 4. Real persistence of native connectors, `open`/`key`/`link`/`local` plugins, and custom
//!    STDIO/HTTP MCP servers into `OpenBotHostRuntime::mcp()` & `openbot-connectors-v1.json`,
//!    plus agent/skill provisioning and per-server access control.

use std::collections::BTreeMap;

use gpui_telegram_desktop::theme::TdesktopThemePalette;
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleAppSidebar, AppleSidebarSection, DEFAULT_PINNED_SECTIONS, PINNABLE_SECTIONS,
    load_page_rail_pins, sanitize_pinned_sections,
};
use gpui_telegram_desktop::views::content_hub::{
    ContentHubSection, ContentHubWorkspace, CustomServerFormDraft, MarketplaceHubState,
    MarketplaceNavTab, MarketplaceRoute,
};
use gpui_telegram_desktop::views::openbot::MarketplacePageState;
use openbot_runtime::OpenBotHostRuntime;
use openbot_runtime::agent_lifecycle::CreateAgentRequest;
use openbot_runtime::platform::marketplace::{MarketplaceTab, openbot_marketplace_catalog};
use openbot_runtime::skill_mcp_service::{RegisterMcpServerInput, SaveSkillInput};
use openbot_storage::{McpKeyValue, McpTransport};

#[test]
fn test_mcp_tools_and_schedule_entries_removed_and_content_verticals_moved_to_marketplace() {
    // 1. Neither McpTools nor Schedule may appear anywhere in default or pinnable rail sections.
    assert!(!DEFAULT_PINNED_SECTIONS.contains(&AppleSidebarSection::McpTools));
    assert!(!DEFAULT_PINNED_SECTIONS.contains(&AppleSidebarSection::Schedule));
    assert!(!PINNABLE_SECTIONS.contains(&AppleSidebarSection::McpTools));
    assert!(!PINNABLE_SECTIONS.contains(&AppleSidebarSection::Schedule));
    assert!(!AppleSidebarSection::McpTools.is_pinnable());
    assert!(!AppleSidebarSection::Schedule.is_pinnable());

    // 2. Books, Games, and AppStore (MiniApps) have moved into Marketplace; Workflows, Community & Music stay in the rail.
    assert_eq!(
        DEFAULT_PINNED_SECTIONS,
        [
            AppleSidebarSection::Workflows,
            AppleSidebarSection::Community,
            AppleSidebarSection::Music,
        ]
    );
    assert!(!PINNABLE_SECTIONS.contains(&AppleSidebarSection::Books));
    assert!(!PINNABLE_SECTIONS.contains(&AppleSidebarSection::Games));
    assert!(!PINNABLE_SECTIONS.contains(&AppleSidebarSection::AppStore));
    assert!(!AppleSidebarSection::Books.is_pinnable());
    assert!(!AppleSidebarSection::Games.is_pinnable());
    assert!(!AppleSidebarSection::AppStore.is_pinnable());

    // 3. Marketplace lives in the bottom rail utility stack right above Help & Settings.
    let rail = AppleAppSidebar::new(TdesktopThemePalette::default(), "zh-CN");
    let layout = rail.compute_layout(900.0);
    assert!(layout.bottom_items.marketplace_visible);
    assert!(layout.bottom_items.help_visible);
    assert!(layout.bottom_items.settings_visible);
    assert_eq!(AppleSidebarSection::Marketplace.localized_label("zh-CN"), "市场");
    assert_eq!(AppleSidebarSection::Marketplace.localized_label("en"), "Marketplace");

    // 4. Legacy pin JSON containing retired or moved entries auto-sanitizes cleanly on load.
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("openbot-page-rail-pins-v1.json"),
        r#"{"version":1,"pinned_sections":["community","books","games","app_store","mcp_tools","schedule","music","browser"]}"#,
    )
    .expect("write legacy pins");
    assert_eq!(
        load_page_rail_pins(tmp.path()),
        vec![
            AppleSidebarSection::Community,
            AppleSidebarSection::Music,
            AppleSidebarSection::Browser,
        ]
    );

    // 5. sanitize_pinned_sections strips all retired & Marketplace-hosted items.
    assert_eq!(
        sanitize_pinned_sections(vec![
            AppleSidebarSection::McpTools,
            AppleSidebarSection::Schedule,
            AppleSidebarSection::Books,
            AppleSidebarSection::Games,
            AppleSidebarSection::AppStore,
            AppleSidebarSection::Marketplace,
            AppleSidebarSection::Workflows,
        ]),
        vec![AppleSidebarSection::Workflows]
    );
}

#[test]
fn test_marketplace_sidebar_separates_connector_apps_from_miniapps_books_and_games() {
    // Primary OpenBot tabs: 应用 (Connectors/MCP), 助手 (Agents), 技能 (Skills)
    assert_eq!(
        MarketplaceNavTab::PRIMARY_TABS,
        [
            MarketplaceNavTab::Apps,
            MarketplaceNavTab::Agents,
            MarketplaceNavTab::Skills,
        ]
    );
    // Separate content/miniapp section placed explicitly below 技能: 小程序应用, 图书, 游戏
    assert_eq!(
        MarketplaceNavTab::MEDIA_STORE_TABS,
        [
            MarketplaceNavTab::MiniApps,
            MarketplaceNavTab::Books,
            MarketplaceNavTab::Games,
        ]
    );
    assert_eq!(MarketplaceNavTab::Apps.localized_label("zh-CN"), "应用");
    assert_eq!(MarketplaceNavTab::Agents.localized_label("zh-CN"), "助手");
    assert_eq!(MarketplaceNavTab::Skills.localized_label("zh-CN"), "技能");
    assert_eq!(MarketplaceNavTab::MiniApps.localized_label("zh-CN"), "小程序应用");
    assert_eq!(MarketplaceNavTab::Books.localized_label("zh-CN"), "图书");
    assert_eq!(MarketplaceNavTab::Games.localized_label("zh-CN"), "游戏");

    // Workspace defaults to Marketplace and maps Books/Games/AppStore deep-links onto their Market tabs.
    let mut ws = ContentHubWorkspace::new(TdesktopThemePalette::default(), "zh-CN");
    assert_eq!(ws.active_section, ContentHubSection::Marketplace);
    assert_eq!(ws.marketplace.active_tab, MarketplaceNavTab::Apps);

    ws.set_section(ContentHubSection::AppStore);
    assert_eq!(ws.marketplace.active_tab, MarketplaceNavTab::MiniApps);

    ws.set_section(ContentHubSection::Books);
    assert_eq!(ws.marketplace.active_tab, MarketplaceNavTab::Books);

    ws.set_section(ContentHubSection::Games);
    assert_eq!(ws.marketplace.active_tab, MarketplaceNavTab::Games);
}

#[test]
fn test_openbot_marketplace_catalog_exact_266_connectors_29_agents_40_skills() {
    let catalog = openbot_marketplace_catalog();
    assert_eq!(catalog.connectors.len(), 9, "must include all 9 native OpenBot connectors");
    assert_eq!(catalog.plugins.len(), 257, "must include all 257 OpenBot plugin-catalog MCP apps");
    assert_eq!(catalog.agents.len(), 29, "must include all 29 OpenBot marketplace agents");
    assert_eq!(catalog.skills.len(), 40, "must include all 40 OpenBot marketplace skills");

    // Verify exact upstream category counts shown in OpenBot Marketplace screenshots
    let state = MarketplaceHubState::new();
    let groups = state.grouped_apps("zh-CN");
    let counts: BTreeMap<&str, usize> = groups
        .iter()
        .map(|(cat, items)| (*cat, items.len()))
        .collect();
    assert_eq!(counts.get("productivity").copied(), Some(84));
    assert_eq!(counts.get("documents").copied(), Some(8));
    assert_eq!(counts.get("design").copied(), Some(25));
    assert_eq!(counts.get("research").copied(), Some(27));
    assert_eq!(counts.get("data-analytics").copied(), Some(40));
    assert_eq!(counts.get("automation").copied(), Some(18));
    assert_eq!(counts.get("coding").copied(), Some(44));
    assert_eq!(counts.get("other").copied(), Some(20));

    let total_apps: usize = counts.values().sum();
    assert_eq!(total_apps, 266);

    // Verify secondary OpenBot MarketplacePageState is also backed by the full 335-item catalog
    let secondary = MarketplacePageState::new(false);
    assert_eq!(secondary.items.len(), 9 + 257 + 29 + 40);
}

#[test]
fn test_connector_and_custom_mcp_persistence_agent_access_and_skill_install_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = OpenBotHostRuntime::open(tmp.path()).expect("open runtime");

    // Start with 0 connected apps & 0 MCP servers
    assert!(rt.marketplace().list_connected_apps().is_empty());
    assert!(rt.mcp().list_servers().expect("list servers").is_empty());

    // 1. Connect native connector (Slack)
    let slack = rt
        .marketplace()
        .connect_native_connector(
            rt.mcp(),
            "slack",
            "Engineering Workspace",
            Some("xoxb-test-token"),
            BTreeMap::new(),
        )
        .expect("connect slack");
    assert_eq!(slack.app_id, "connector:slack");
    assert!(slack.mcp_server_id.is_some());

    // 2. Connect an open (zero-auth) plugin (Context7) and an API-key plugin
    let open_slug = openbot_marketplace_catalog()
        .plugins
        .iter()
        .find(|p| p.auth_kind == "open")
        .expect("open plugin exists")
        .slug
        .clone();
    let (open_rec, open_srv) = rt
        .marketplace()
        .connect_plugin_via_mcp(rt.mcp(), &open_slug, &BTreeMap::new(), None)
        .expect("connect open plugin");
    assert_eq!(open_rec.mcp_server_id.as_deref(), Some(open_srv.id.as_str()));

    // 3. Add a custom Streamable HTTP MCP server via CustomServerFormDraft parsing
    let mut form = CustomServerFormDraft::default();
    form.transport = McpTransport::Http;
    form.name.insert_text("Internal Docs MCP");
    form.url.insert_text("https://mcp.internal.example/v1");
    form.header_lines
        .insert_text("Authorization: Bearer secret-123\nX-Env: prod");
    let custom_srv = rt
        .mcp()
        .register_server(RegisterMcpServerInput {
            id: None,
            name: form.name.text.clone(),
            transport: form.transport,
            enabled: Some(true),
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            env_passthrough: Vec::new(),
            working_directory: None,
            url: Some(form.url.text.clone()),
            headers: form.parsed_headers(),
        })
        .expect("register custom mcp");
    assert_eq!(custom_srv.headers.len(), 2);

    // 4. Sync into MarketplaceHubState -> `your_connected_apps()` reflects all 3 live connectors/MCPs!
    let mut ui = MarketplaceHubState::new();
    ui.connected_apps = rt.marketplace().list_connected_apps();
    ui.mcp_servers = rt.mcp().list_servers().expect("list servers");
    assert_eq!(ui.mcp_servers.len(), 3);
    assert_eq!(ui.your_connected_apps().len(), 3);
    assert!(ui.is_app_connected("connector:slack"));
    assert!(ui.is_app_connected(&open_rec.app_id));
    assert!(ui.is_app_connected(&format!("custom:{}", custom_srv.id)));

    // 5. Provision a Marketplace agent + install its bundled skills + toggle per-server access
    let agent = rt
        .agents()
        .create_agent_full(CreateAgentRequest {
            name: "Researcher".into(),
            title: "marketplace:researcher".into(),
            description: "Deep research assistant".into(),
            allow_automation: Some(true),
            computer_use: Some(true),
            ..Default::default()
        })
        .expect("create agent");

    let skill = &openbot_marketplace_catalog().skills[0];
    rt.skills()
        .save_skill(SaveSkillInput {
            agent_id: agent.id.clone(),
            slug: skill.slug.clone(),
            name: skill.name.clone(),
            description: skill.description.clone(),
            instructions: skill.skill_md.clone(),
            triggers: vec![format!("${}", skill.slug)],
            tools: Vec::new(),
            origin: Some("marketplace".into()),
            enabled: Some(true),
        })
        .expect("install skill");
    let _ = rt
        .marketplace()
        .install_item(MarketplaceTab::Skills, &skill.slug);

    assert_eq!(
        rt.skills().list_skills(&agent.id).expect("skills").len(),
        1
    );

    // Restrict custom_srv access away from agent and verify list_server_access reflects it
    rt.mcp()
        .set_server_access(&custom_srv.id, Some(Vec::new()))
        .expect("set server access");
    assert!(!rt.mcp().allows_agent(&custom_srv.id, &agent.id));
    let access_map = rt.mcp().list_server_access();
    assert_eq!(access_map.get(&custom_srv.id), Some(&Some(Vec::new())));

    // 6. Disconnect native connector & remove custom server -> cleaned from MCP gateway too
    rt.marketplace()
        .disconnect_app_via_mcp(rt.mcp(), "connector:slack")
        .expect("disconnect slack");
    rt.mcp()
        .remove_server(&custom_srv.id)
        .expect("remove custom");
    assert_eq!(rt.mcp().list_servers().expect("list servers").len(), 1);
    assert_eq!(rt.marketplace().list_connected_apps().len(), 1);

    // 7. Re-open runtime from disk and verify remaining connected app persisted across restarts
    drop(rt);
    let rt2 = OpenBotHostRuntime::open(tmp.path()).expect("reopen runtime");
    assert_eq!(rt2.marketplace().list_connected_apps().len(), 1);
    assert_eq!(rt2.mcp().list_servers().expect("list servers").len(), 1);

    // Sanity check breadcrumb route navigation on MarketplaceHubState
    ui.push_route(MarketplaceRoute::AppDetail {
        app_id: open_rec.app_id.clone(),
    });
    assert_eq!(ui.stack.len(), 2);
    assert!(ui.pop_route());
    assert_eq!(ui.stack.len(), 1);
    let _ = McpKeyValue {
        key: "K".into(),
        value: "V".into(),
    };
}

#[test]
fn test_zero_fake_install_counts_zero_fake_endpoints_and_published_apps_surface() {
    let catalog = openbot_marketplace_catalog();

    // 1. Every single catalog item starts at 0 installs (zero fabricated counts).
    assert!(
        catalog.connectors.iter().all(|c| c.installs == 0),
        "native connectors must not have hardcoded fake install counts"
    );
    assert!(
        catalog.plugins.iter().all(|p| p.installs == 0),
        "plugins must not have hardcoded fake install counts"
    );
    assert!(
        catalog.agents.iter().all(|a| a.installs == 0),
        "agents must not have hardcoded fake install counts"
    );
    assert!(
        catalog.skills.iter().all(|s| s.installs == 0),
        "skills must not have hardcoded fake install counts"
    );

    // 2. Native connector MCP registration uses authentic upstream HTTPS endpoints or real CLI commands,
    //    never synthetic `connectors.openbot.local` placeholder URLs.
    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = OpenBotHostRuntime::open(tmp.path()).expect("open runtime");
    let slack = rt
        .marketplace()
        .connect_native_connector(rt.mcp(), "slack", "Acme", Some("xoxb-test"), BTreeMap::new())
        .expect("connect slack");
    assert_eq!(slack.endpoint.as_deref(), Some("https://slack.com/api"));
    let servers = rt.mcp().list_servers().expect("list servers");
    assert!(
        servers
            .iter()
            .all(|s| !s.url.contains("connectors.openbot.local")),
        "must never register synthetic connectors.openbot.local URLs"
    );

    // 3. Unverified status (`needs_sign_in`, `error`) must NEVER report as connected.
    let mut ui = MarketplaceHubState::new();
    let mut unverified = slack.clone();
    unverified.status = "needs_sign_in".to_string();
    ui.connected_apps = vec![unverified];
    assert!(
        !ui.is_app_connected("connector:slack"),
        "needs_sign_in record must NOT appear connected"
    );

    // 4. Server-published custom connectors appear seamlessly in `all_apps()` and `find_app()`.
    let published_dto = gpui_telegram_proto::marketplace::MarketplaceAppListingDto {
        app_id: "published:acme-billing".into(),
        slug: "acme-billing".into(),
        kind: "custom_published".into(),
        name: "Acme Billing MCP".into(),
        tagline: "Live billing & invoices".into(),
        zh_tagline: Some("实时账单与发票查询".into()),
        description: "Connect to Acme Billing Streamable HTTP MCP".into(),
        zh_description: None,
        category: "data-analytics".into(),
        creator_name: "alice".into(),
        version: "1.0.0".into(),
        brand_hex: "#0A84FF".into(),
        website_url: None,
        privacy_policy_url: None,
        terms_url: None,
        server_name: "acme-billing".into(),
        transport: "http".into(),
        url: Some("https://billing.acme.example/mcp".into()),
        command: None,
        args: Vec::new(),
        env: Vec::new(),
        auth_kind: "open".into(),
        auth_label: None,
        auth_fields: Vec::new(),
        auth_steps: Vec::new(),
        auth_note: None,
        auth_docs_url: None,
        auth_docs_label: None,
        prompts: Vec::new(),
        setup_kind: None,
        verified: false,
        install_count: 0,
        publisher_user_id: Some(1),
        published_at_ms: 1_700_000_000_000,
    };
    ui.published_custom_apps.push(published_dto);
    assert_eq!(ui.all_apps().len(), 267);
    assert!(ui.find_app("published:acme-billing").is_some());
}

