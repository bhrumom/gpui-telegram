//! State model & pure catalog filtering for the unified OpenBot 1:1 Marketplace
//! (`应用 / 助手 / 技能` + separated `小程序应用 / 图书 / 游戏` entries below `技能`).

use std::collections::{BTreeMap, HashSet};

use openbot_runtime::platform::marketplace::{
    ConnectedAppRecord, OpenBotAgentCatalogItem, OpenBotCatalog, OpenBotConnectorCatalogItem,
    OpenBotPluginCatalogItem, OpenBotSkillCatalogItem, SKILL_CATEGORIES,
    openbot_marketplace_catalog,
};
use openbot_storage::{McpKeyValue, McpServerConfig, McpTransport};

use crate::i18n::tr_str;
use crate::views::content_hub::EditableFieldState;

/// Top-level Marketplace navigation entries in the left `260px` sidebar.
///
/// The primary three (`Apps`, `Agents`, `Skills`) mirror OpenBot's `MARKETPLACE_TABS` 1:1.
/// Below `Skills`, `MiniApps`, `Books`, and `Games` live as separate entries so Mini-App Store
/// apps are cleanly distinguished from Connector Apps (`Apps`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MarketplaceNavTab {
    #[default]
    Apps,
    Agents,
    Skills,
    MiniApps,
    Books,
    Games,
}

impl MarketplaceNavTab {
    pub const PRIMARY_TABS: [Self; 3] = [Self::Apps, Self::Agents, Self::Skills];
    pub const MEDIA_STORE_TABS: [Self; 3] = [Self::MiniApps, Self::Books, Self::Games];

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Agents => "agents",
            Self::Skills => "skills",
            Self::MiniApps => "mini_apps",
            Self::Books => "books",
            Self::Games => "games",
        }
    }

    pub fn localized_label(self, lang: &str) -> &'static str {
        match self {
            Self::Apps => tr_str(lang, "应用", "Apps"),
            Self::Agents => tr_str(lang, "助手", "Agents"),
            Self::Skills => tr_str(lang, "技能", "Skills"),
            Self::MiniApps => tr_str(lang, "小程序应用", "Mini Apps"),
            Self::Books => tr_str(lang, "图书", "Books"),
            Self::Games => tr_str(lang, "游戏", "Games"),
        }
    }
}

/// Source trust filter matching OpenBot `SourceSwitch` (`全部 | 已验证 | 公开`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MarketplaceSourceChoice {
    #[default]
    All,
    Verified,
    Public,
}

/// Skill sort mode matching OpenBot `MarketplaceSkillSort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MarketplaceSkillSortMode {
    #[default]
    Installs,
    Updated,
    Name,
}

/// Breadcrumb stack route mirroring OpenBot `MarketplaceView`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceRoute {
    Browse,
    AppGroup { category: String },
    AppDetail { app_id: String },
    CustomServerForm { edit_server_id: Option<String> },
    AgentGroup { category: String },
    AgentDetail { slug: String },
    SkillGroup { group: String },
    SkillDetail { slug: String },
}

/// Live assistant/bot summary synchronized from `AppShell` + `OpenBotHostRuntime`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceBotSummary {
    pub openbot_agent_id: String,
    pub fabushi_agent_id: Option<String>,
    pub username: Option<String>,
    pub name: String,
    pub title: String,
    pub avatar_shape: String,
    pub avatar_color: String,
    pub installed_skill_slugs: Vec<String>,
}

/// Unified connector/plugin/custom app item shown in the Marketplace `应用` tab.
#[derive(Debug, Clone)]
pub enum MarketplaceAppEntry {
    Connector(&'static OpenBotConnectorCatalogItem),
    Plugin(&'static OpenBotPluginCatalogItem),
    Custom(McpServerConfig),
}

impl MarketplaceAppEntry {
    pub fn id(&self) -> String {
        match self {
            Self::Connector(c) => c.id.clone(),
            Self::Plugin(p) => p.id.clone(),
            Self::Custom(s) => format!("custom:{}", s.id),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Connector(c) => &c.name,
            Self::Plugin(p) => &p.name,
            Self::Custom(s) => &s.name,
        }
    }

    pub fn category(&self) -> &str {
        match self {
            Self::Connector(c) => &c.category,
            Self::Plugin(p) => &p.category,
            Self::Custom(_) => "other",
        }
    }

    pub fn localized_tagline(&self, lang: &str) -> String {
        let is_zh = lang.starts_with("zh");
        match self {
            Self::Connector(c) => {
                if is_zh && !c.zh_tagline.is_empty() {
                    c.zh_tagline.clone()
                } else {
                    c.tagline.clone()
                }
            }
            Self::Plugin(p) => {
                if is_zh {
                    p.zh_tagline
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&p.tagline)
                        .to_string()
                } else {
                    p.tagline.clone()
                }
            }
            Self::Custom(s) => match s.transport {
                McpTransport::Stdio => {
                    if s.command.trim().is_empty() {
                        tr_str(lang, "自定义 STDIO MCP 服务器", "Custom STDIO MCP server").to_string()
                    } else {
                        format!("STDIO · {}", s.command.trim())
                    }
                }
                McpTransport::Http => {
                    if s.url.trim().is_empty() {
                        tr_str(lang, "自定义 HTTP MCP 服务器", "Custom HTTP MCP server").to_string()
                    } else {
                        s.url.trim().to_string()
                    }
                }
            },
        }
    }

    pub fn localized_description(&self, lang: &str) -> String {
        let is_zh = lang.starts_with("zh");
        match self {
            Self::Connector(c) => {
                if is_zh && !c.zh_description.is_empty() {
                    c.zh_description.clone()
                } else {
                    c.description.clone()
                }
            }
            Self::Plugin(p) => {
                if is_zh {
                    p.zh_description
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&p.description)
                        .to_string()
                } else {
                    p.description.clone()
                }
            }
            Self::Custom(_) => tr_str(
                lang,
                "通过自定义 STDIO 或 Streamable HTTP 接入的 MCP 工具服务器。",
                "Custom MCP tool server connected over STDIO or Streamable HTTP.",
            )
            .to_string(),
        }
    }

    pub fn brand_hex(&self) -> &str {
        match self {
            Self::Connector(c) => &c.brand_hex,
            Self::Plugin(p) => &p.brand_hex,
            Self::Custom(_) => "#3F3F46",
        }
    }
}

/// Interactive credential/setup draft for connecting a native connector or plugin app.
#[derive(Debug, Clone, Default)]
pub struct ConnectorSetupDraft {
    pub app_id: String,
    pub primary: EditableFieldState,
    pub secondary: EditableFieldState,
    pub extra: Vec<(String, EditableFieldState)>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

/// Complete `+ 添加自定义项` MCP server form draft mirroring OpenBot `McpServerForm.tsx`.
#[derive(Debug, Clone)]
pub struct CustomServerFormDraft {
    pub edit_server_id: Option<String>,
    pub transport: McpTransport,
    pub name: EditableFieldState,
    pub command: EditableFieldState,
    pub args: EditableFieldState,
    pub env_lines: EditableFieldState,
    pub working_dir: EditableFieldState,
    pub url: EditableFieldState,
    pub header_lines: EditableFieldState,
    pub error: Option<String>,
    pub test_result: Option<(bool, String)>,
}

impl Default for CustomServerFormDraft {
    fn default() -> Self {
        Self {
            edit_server_id: None,
            transport: McpTransport::Http,
            name: EditableFieldState::default(),
            command: EditableFieldState::default(),
            args: EditableFieldState::default(),
            env_lines: EditableFieldState::default(),
            working_dir: EditableFieldState::default(),
            url: EditableFieldState::default(),
            header_lines: EditableFieldState::default(),
            error: None,
            test_result: None,
        }
    }
}

impl CustomServerFormDraft {
    pub fn reset_new(&mut self) {
        *self = Self::default();
    }

    pub fn load_from_server(&mut self, srv: &McpServerConfig) {
        self.edit_server_id = Some(srv.id.clone());
        self.transport = srv.transport;
        self.name = EditableFieldState::with_text(&srv.name);
        self.command = EditableFieldState::with_text(&srv.command);
        self.args = EditableFieldState::with_text(srv.args.join(" "));
        let env_str = srv
            .env
            .iter()
            .map(|p| format!("{}={}", p.key, p.value))
            .collect::<Vec<_>>()
            .join("\n");
        self.env_lines = EditableFieldState::with_text(env_str);
        self.working_dir = EditableFieldState::with_text(&srv.working_directory);
        self.url = EditableFieldState::with_text(&srv.url);
        let hdr_str = srv
            .headers
            .iter()
            .map(|p| format!("{}: {}", p.key, p.value))
            .collect::<Vec<_>>()
            .join("\n");
        self.header_lines = EditableFieldState::with_text(hdr_str);
        self.error = None;
        self.test_result = None;
    }

    pub fn parsed_args(&self) -> Vec<String> {
        self.args
            .text
            .split_whitespace()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    pub fn parsed_env(&self) -> Vec<McpKeyValue> {
        parse_key_value_lines(&self.env_lines.text, '=')
    }

    pub fn parsed_headers(&self) -> Vec<McpKeyValue> {
        parse_key_value_lines(&self.header_lines.text, ':')
    }

    pub fn clear_all_preedits(&mut self) {
        self.name.set_ime_preedit(None);
        self.command.set_ime_preedit(None);
        self.args.set_ime_preedit(None);
        self.env_lines.set_ime_preedit(None);
        self.working_dir.set_ime_preedit(None);
        self.url.set_ime_preedit(None);
        self.header_lines.set_ime_preedit(None);
    }
}

pub fn parse_key_value_lines(raw: &str, sep: char) -> Vec<McpKeyValue> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(sep) {
            let key = k.trim();
            if !key.is_empty() {
                out.push(McpKeyValue {
                    key: key.to_string(),
                    value: v.trim().to_string(),
                });
            }
        }
    }
    out
}

/// Complete OpenBot Marketplace state owned by `ContentHubWorkspace`.
#[derive(Debug, Clone)]
pub struct MarketplaceHubState {
    pub active_tab: MarketplaceNavTab,
    pub stack: Vec<MarketplaceRoute>,
    pub search: EditableFieldState,
    pub agent_source: MarketplaceSourceChoice,
    pub skill_source: MarketplaceSourceChoice,
    pub skill_sort: MarketplaceSkillSortMode,
    pub connected_apps: Vec<ConnectedAppRecord>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub server_access: BTreeMap<String, Option<Vec<String>>>,
    pub bots: Vec<MarketplaceBotSummary>,
    pub added_agent_slugs: HashSet<String>,
    pub skill_picker_slug: Option<String>,
    pub setup_draft: Option<ConnectorSetupDraft>,
    pub custom_form: CustomServerFormDraft,
    pub notice_banner: Option<String>,
    pub error_banner: Option<String>,
}

impl Default for MarketplaceHubState {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketplaceHubState {
    pub fn new() -> Self {
        Self {
            active_tab: MarketplaceNavTab::Apps,
            stack: vec![MarketplaceRoute::Browse],
            search: EditableFieldState::default(),
            agent_source: MarketplaceSourceChoice::All,
            skill_source: MarketplaceSourceChoice::All,
            skill_sort: MarketplaceSkillSortMode::Installs,
            connected_apps: Vec::new(),
            mcp_servers: Vec::new(),
            server_access: BTreeMap::new(),
            bots: Vec::new(),
            added_agent_slugs: HashSet::new(),
            skill_picker_slug: None,
            setup_draft: None,
            custom_form: CustomServerFormDraft::default(),
            notice_banner: None,
            error_banner: None,
        }
    }

    pub fn catalog(&self) -> &'static OpenBotCatalog {
        openbot_marketplace_catalog()
    }

    pub fn current_route(&self) -> &MarketplaceRoute {
        self.stack.last().unwrap_or(&MarketplaceRoute::Browse)
    }

    pub fn select_tab(&mut self, tab: MarketplaceNavTab) {
        self.active_tab = tab;
        self.stack = vec![MarketplaceRoute::Browse];
        self.skill_picker_slug = None;
        self.setup_draft = None;
        self.error_banner = None;
    }

    pub fn push_route(&mut self, route: MarketplaceRoute) {
        self.stack.push(route);
        self.skill_picker_slug = None;
        self.error_banner = None;
    }

    pub fn back_to(&mut self, index: usize) {
        let keep = (index + 1).max(1).min(self.stack.len());
        self.stack.truncate(keep);
        self.skill_picker_slug = None;
        self.error_banner = None;
    }

    pub fn pop_route(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.skill_picker_slug = None;
            self.error_banner = None;
            true
        } else {
            false
        }
    }

    pub fn has_any_preedit(&self) -> bool {
        self.search.ime_preedit.is_some()
            || self.setup_draft.as_ref().is_some_and(|d| {
                d.primary.ime_preedit.is_some()
                    || d.secondary.ime_preedit.is_some()
                    || d.extra.iter().any(|(_, f)| f.ime_preedit.is_some())
            })
            || self.custom_form.name.ime_preedit.is_some()
            || self.custom_form.command.ime_preedit.is_some()
            || self.custom_form.args.ime_preedit.is_some()
            || self.custom_form.env_lines.ime_preedit.is_some()
            || self.custom_form.working_dir.ime_preedit.is_some()
            || self.custom_form.url.ime_preedit.is_some()
            || self.custom_form.header_lines.ime_preedit.is_some()
    }

    pub fn clear_all_preedits(&mut self) {
        self.search.set_ime_preedit(None);
        if let Some(ref mut d) = self.setup_draft {
            d.primary.set_ime_preedit(None);
            d.secondary.set_ime_preedit(None);
            for (_, f) in &mut d.extra {
                f.set_ime_preedit(None);
            }
        }
        self.custom_form.clear_all_preedits();
    }

    pub fn is_app_connected(&self, app_id: &str) -> bool {
        if let Some(custom_id) = app_id.strip_prefix("custom:") {
            return self.mcp_servers.iter().any(|s| s.id == custom_id && s.enabled);
        }
        self.connected_apps.iter().any(|r| r.app_id == app_id)
    }

    pub fn connected_record(&self, app_id: &str) -> Option<&ConnectedAppRecord> {
        self.connected_apps.iter().find(|r| r.app_id == app_id)
    }

    pub fn linked_mcp_server_id(&self, app_id: &str) -> Option<String> {
        if let Some(custom_id) = app_id.strip_prefix("custom:") {
            return Some(custom_id.to_string());
        }
        self.connected_record(app_id)
            .and_then(|r| r.mcp_server_id.clone())
    }

    /// Returns all apps across native connectors (9), OpenBot plugins (257), and user-added custom
    /// MCP servers (excluding servers already owned by a connected connector or plugin).
    pub fn all_apps(&self) -> Vec<MarketplaceAppEntry> {
        let cat = self.catalog();
        let managed_server_ids: HashSet<&str> = self
            .connected_apps
            .iter()
            .filter_map(|r| r.mcp_server_id.as_deref())
            .collect();

        let mut out =
            Vec::with_capacity(cat.connectors.len() + cat.plugins.len() + self.mcp_servers.len());
        for c in &cat.connectors {
            out.push(MarketplaceAppEntry::Connector(c));
        }
        for p in &cat.plugins {
            out.push(MarketplaceAppEntry::Plugin(p));
        }
        for srv in &self.mcp_servers {
            if !managed_server_ids.contains(srv.id.as_str()) {
                out.push(MarketplaceAppEntry::Custom(srv.clone()));
            }
        }
        out
    }

    pub fn find_app(&self, app_id: &str) -> Option<MarketplaceAppEntry> {
        self.all_apps().into_iter().find(|a| a.id() == app_id)
    }

    /// Returns connected apps & custom MCP servers surfaced under `你的应用` (`Your apps`) in sidebar.
    pub fn your_connected_apps(&self) -> Vec<MarketplaceAppEntry> {
        self.all_apps()
            .into_iter()
            .filter(|a| match a {
                MarketplaceAppEntry::Custom(_) => true,
                other => self.is_app_connected(&other.id()),
            })
            .collect()
    }

    pub fn filtered_apps(&self, lang: &str) -> Vec<MarketplaceAppEntry> {
        let q = self.search.text.trim().to_lowercase();
        self.all_apps()
            .into_iter()
            .filter(|app| {
                if q.is_empty() {
                    return true;
                }
                app.name().to_lowercase().contains(&q)
                    || app.localized_tagline(lang).to_lowercase().contains(&q)
                    || app.category().to_lowercase().contains(&q)
                    || category_localized_label(app.category(), lang)
                        .to_lowercase()
                        .contains(&q)
            })
            .collect()
    }

    /// Groups apps by OpenBot's canonical `SKILL_CATEGORIES` order.
    pub fn grouped_apps(&self, lang: &str) -> Vec<(&'static str, Vec<MarketplaceAppEntry>)> {
        let apps = self.filtered_apps(lang);
        let mut groups = Vec::new();
        for &cat in &SKILL_CATEGORIES {
            let bucket: Vec<MarketplaceAppEntry> = apps
                .iter()
                .filter(|a| a.category().eq_ignore_ascii_case(cat))
                .cloned()
                .collect();
            if !bucket.is_empty() {
                groups.push((cat, bucket));
            }
        }
        groups
    }

    pub fn filtered_agents(&self) -> Vec<&'static OpenBotAgentCatalogItem> {
        let q = self.search.text.trim().to_lowercase();
        self.catalog()
            .agents
            .iter()
            .filter(|a| match self.agent_source {
                MarketplaceSourceChoice::All => true,
                MarketplaceSourceChoice::Verified => a.verified,
                MarketplaceSourceChoice::Public => !a.verified,
            })
            .filter(|a| {
                q.is_empty()
                    || a.name.to_lowercase().contains(&q)
                    || a.title.to_lowercase().contains(&q)
                    || a.description.to_lowercase().contains(&q)
                    || a.category.to_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn is_agent_added(&self, agent: &OpenBotAgentCatalogItem) -> bool {
        if self.added_agent_slugs.contains(&agent.slug) {
            return true;
        }
        let target_name = agent.name.trim().to_lowercase();
        self.bots.iter().any(|b| {
            b.name.trim().to_lowercase() == target_name
                || b.title.contains(&format!("marketplace:{}", agent.slug))
        })
    }

    pub fn filtered_skills(&self) -> Vec<&'static OpenBotSkillCatalogItem> {
        let q = self.search.text.trim().to_lowercase();
        let mut list: Vec<&'static OpenBotSkillCatalogItem> = self
            .catalog()
            .skills
            .iter()
            .filter(|s| match self.skill_source {
                MarketplaceSourceChoice::All => true,
                MarketplaceSourceChoice::Verified => s.verified,
                MarketplaceSourceChoice::Public => !s.verified,
            })
            .filter(|s| {
                q.is_empty()
                    || s.name.to_lowercase().contains(&q)
                    || s.slug.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
                    || s.category.to_lowercase().contains(&q)
            })
            .collect();
        match self.skill_sort {
            MarketplaceSkillSortMode::Installs => {
                list.sort_by(|a, b| b.installs.cmp(&a.installs).then_with(|| a.name.cmp(&b.name)));
            }
            MarketplaceSkillSortMode::Updated => {
                list.sort_by(|a, b| b.version.cmp(&a.version).then_with(|| b.installs.cmp(&a.installs)));
            }
            MarketplaceSkillSortMode::Name => {
                list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
        }
        list
    }

    pub fn agents_with_skill(&self, slug: &str) -> Vec<&MarketplaceBotSummary> {
        self.bots
            .iter()
            .filter(|b| {
                b.installed_skill_slugs
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(slug))
            })
            .collect()
    }

    pub fn open_setup_for_app(&mut self, app_id: &str) {
        let Some(app) = self.find_app(app_id) else {
            return;
        };
        let existing = self.connected_record(app_id).cloned();
        let mut draft = ConnectorSetupDraft {
            app_id: app_id.to_string(),
            ..Default::default()
        };
        match app {
            MarketplaceAppEntry::Connector(c) => {
                if let Some(ref rec) = existing {
                    if let Some(ref lbl) = rec.account_label {
                        draft.primary = EditableFieldState::with_text(lbl);
                    }
                } else {
                    draft.primary = EditableFieldState::default();
                }
                if c.setup_kind == "vault" {
                    if let Some(ref rec) = existing {
                        if let Some(v) = rec.metadata.get("vault") {
                            draft.secondary = EditableFieldState::with_text(v);
                        }
                    }
                }
            }
            MarketplaceAppEntry::Plugin(p) => {
                if p.auth_kind == "local" {
                    let default_url = existing
                        .as_ref()
                        .and_then(|r| r.account_label.clone())
                        .or_else(|| p.url.clone())
                        .unwrap_or_default();
                    draft.primary = EditableFieldState::with_text(default_url);
                } else if !p.auth_fields.is_empty() {
                    for (idx, field) in p.auth_fields.iter().enumerate() {
                        if idx == 0 {
                            draft.primary = EditableFieldState::default();
                        } else if idx == 1 {
                            draft.secondary = EditableFieldState::default();
                        } else {
                            draft.extra.push((field.id.clone(), EditableFieldState::default()));
                        }
                    }
                }
            }
            MarketplaceAppEntry::Custom(_) => {}
        }
        self.setup_draft = Some(draft);
    }
}

/// Canonical localized label for each of the 8 OpenBot Marketplace categories.
pub fn category_localized_label(category: &str, lang: &str) -> &'static str {
    match category.trim().to_ascii_lowercase().as_str() {
        "productivity" => tr_str(lang, "效率", "Productivity"),
        "documents" => tr_str(lang, "文档", "Documents"),
        "design" => tr_str(lang, "设计", "Design"),
        "research" => tr_str(lang, "研究", "Research"),
        "data-analytics" => tr_str(lang, "数据与分析", "Data & Analytics"),
        "automation" => tr_str(lang, "自动化", "Automation"),
        "coding" => tr_str(lang, "编程", "Coding"),
        _ => tr_str(lang, "其他", "Other"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marketplace_state_groups_match_openbot_upstream_counts() {
        let state = MarketplaceHubState::new();
        let groups = state.grouped_apps("zh-CN");
        let count_for = |cat: &str| {
            groups
                .iter()
                .find(|(c, _)| *c == cat)
                .map(|(_, items)| items.len())
                .unwrap_or(0)
        };
        assert_eq!(count_for("productivity"), 84);
        assert_eq!(count_for("documents"), 8);
        assert_eq!(count_for("design"), 25);
        assert_eq!(count_for("research"), 27);
        assert_eq!(count_for("data-analytics"), 40);
        assert_eq!(count_for("automation"), 18);
        assert_eq!(count_for("coding"), 44);
        assert_eq!(count_for("other"), 20);
        assert_eq!(state.all_apps().len(), 266);
        assert_eq!(state.filtered_agents().len(), 29);
        assert_eq!(state.filtered_skills().len(), 40);
    }

    #[test]
    fn marketplace_nav_separates_connector_apps_from_miniapps_books_games_below_skills() {
        assert_eq!(
            MarketplaceNavTab::PRIMARY_TABS,
            [
                MarketplaceNavTab::Apps,
                MarketplaceNavTab::Agents,
                MarketplaceNavTab::Skills,
            ]
        );
        assert_eq!(
            MarketplaceNavTab::MEDIA_STORE_TABS,
            [
                MarketplaceNavTab::MiniApps,
                MarketplaceNavTab::Books,
                MarketplaceNavTab::Games,
            ]
        );
    }
}
