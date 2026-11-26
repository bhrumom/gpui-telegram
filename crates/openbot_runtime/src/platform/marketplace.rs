//! 3-tab OpenBot Marketplace catalog (`apps`, `agents`, `skills`), search/filter engine,
//! `"fishing"` empty-state fallback, and atomic install receipts (`MarketplaceService`, `OB-F12`).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use openbot_storage::write_json_file_atomically;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const MARKETPLACE_TABS: [&str; 3] = ["apps", "agents", "skills"];
pub const SKILL_CATEGORIES: [&str; 8] = [
    "productivity",
    "documents",
    "design",
    "research",
    "data-analytics",
    "automation",
    "coding",
    "other",
];
pub const EMPTY_STATE_ALPHA_FALLBACK: &str = "fishing";
pub const FISHING_VIDEO_ASSET: &str = "fishing.webm";
pub const FISHING_STILL_ASSET: &str = "fishing-still.webp";

const INSTALLS_FILE_NAME: &str = "marketplace-installs-v1.json";
const INSTALLED_SKILLS_FILE_NAME: &str = "openbot-installed-skills-v1.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceTab {
    Apps,
    Agents,
    Skills,
}

impl MarketplaceTab {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Agents => "agents",
            Self::Skills => "skills",
        }
    }

    fn parse(raw: &str) -> RuntimeResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "apps" => Ok(Self::Apps),
            "agents" => Ok(Self::Agents),
            "skills" => Ok(Self::Skills),
            other => Err(OpenBotRuntimeError::Validation(format!(
                "Invalid marketplace tab '{other}'; expected one of apps, agents, skills."
            ))),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSearchRequest {
    pub tab: String,
    pub source: Option<String>,
    pub query: Option<String>,
    pub category: Option<String>,
    pub reduced_motion: bool,
    pub alpha_unsupported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceItem {
    pub id: String,
    pub slug: String,
    pub tab: MarketplaceTab,
    pub name: String,
    pub tagline: String,
    pub description: String,
    pub category: String,
    pub creator_name: String,
    pub verified: bool,
    pub featured: bool,
    pub installs: u64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSearchOutcome {
    pub tab: String,
    pub tabs: Vec<String>,
    pub source: Option<String>,
    pub query: Option<String>,
    pub items: Vec<MarketplaceItem>,
    pub empty_state: bool,
    pub alpha_fallback: String,
    pub plush_asset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallReceipt {
    id: String,
    slug: String,
    tab: MarketplaceTab,
    name: String,
    installed_at: String,
    count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketplaceInstallsFile {
    version: u32,
    installs: BTreeMap<String, InstallReceipt>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalInstalledSkillsFile {
    #[serde(default)]
    by_agent: BTreeMap<String, Vec<LocalInstalledSkillEntry>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalInstalledSkillEntry {
    skill_id: String,
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
}

pub struct MarketplaceService {
    data_dir: PathBuf,
    installs_path: PathBuf,
    installs: Mutex<MarketplaceInstallsFile>,
}

impl MarketplaceService {
    pub fn open(data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let dir = data_dir.as_ref().to_path_buf();
        let installs_path = dir.join(INSTALLS_FILE_NAME);
        let installs = load_installs_file(&installs_path)?;
        Ok(Arc::new(Self {
            data_dir: dir,
            installs_path,
            installs: Mutex::new(installs),
        }))
    }

    pub fn search(&self, req: MarketplaceSearchRequest) -> RuntimeResult<MarketplaceSearchOutcome> {
        let tab = MarketplaceTab::parse(&req.tab)?;
        let normalized_source = match req
            .source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => None,
            Some(s) if s.eq_ignore_ascii_case("verified") => Some("verified".to_string()),
            Some(s) if s.eq_ignore_ascii_case("public") => Some("public".to_string()),
            Some(other) => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid marketplace source '{other}'; expected 'verified' or 'public'."
                )));
            }
        };

        let normalized_category = req
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        if let Some(cat) = normalized_category.as_deref()
            && !SKILL_CATEGORIES.contains(&cat)
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Unknown marketplace category '{cat}'; expected one of: {}.",
                SKILL_CATEGORIES.join(", ")
            )));
        }

        let cleaned_query = req
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let query_lower = cleaned_query.as_deref().map(str::to_lowercase);

        let mut catalog = self.catalog_for_tab(tab);
        {
            let guard = self.installs.lock();
            for item in &mut catalog {
                let key = install_key(item.tab, &item.id);
                if let Some(receipt) = guard.installs.get(&key) {
                    item.installs = item.installs.saturating_add(receipt.count);
                }
            }
        }

        let items: Vec<MarketplaceItem> = catalog
            .into_iter()
            .filter(|item| match normalized_source.as_deref() {
                Some("verified") => item.verified,
                Some("public") => !item.verified,
                _ => true,
            })
            .filter(|item| match normalized_category.as_deref() {
                Some(cat) => item.category.eq_ignore_ascii_case(cat),
                None => true,
            })
            .filter(|item| match query_lower.as_deref() {
                Some(q) => item_matches_query(item, q),
                None => true,
            })
            .collect();

        let empty_state = items.is_empty();
        let plush_asset = if req.reduced_motion || req.alpha_unsupported {
            FISHING_STILL_ASSET.to_string()
        } else {
            FISHING_VIDEO_ASSET.to_string()
        };

        Ok(MarketplaceSearchOutcome {
            tab: tab.as_str().to_string(),
            tabs: MARKETPLACE_TABS.iter().map(|t| (*t).to_string()).collect(),
            source: normalized_source,
            query: cleaned_query,
            items,
            empty_state,
            alpha_fallback: EMPTY_STATE_ALPHA_FALLBACK.to_string(),
            plush_asset,
        })
    }

    pub fn install_item(
        &self,
        tab: MarketplaceTab,
        id_or_slug: &str,
    ) -> RuntimeResult<MarketplaceItem> {
        let needle = id_or_slug.trim();
        if needle.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Marketplace item id or slug must not be empty.".to_string(),
            ));
        }

        let mut item = self
            .catalog_for_tab(tab)
            .into_iter()
            .find(|cand| {
                cand.id.eq_ignore_ascii_case(needle) || cand.slug.eq_ignore_ascii_case(needle)
            })
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "Marketplace {} item '{}' was not found.",
                    tab.as_str(),
                    needle
                ))
            })?;

        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let key = install_key(tab, &item.id);
        let mut guard = self.installs.lock();
        guard.version = 1;
        let entry = guard.installs.entry(key).or_insert_with(|| InstallReceipt {
            id: item.id.clone(),
            slug: item.slug.clone(),
            tab,
            name: item.name.clone(),
            installed_at: now.clone(),
            count: 0,
        });
        entry.installed_at = now;
        entry.count = entry.count.saturating_add(1);
        item.installs = item.installs.saturating_add(entry.count);
        write_json_file_atomically(&self.installs_path, &*guard, true)?;
        Ok(item)
    }

    fn catalog_for_tab(&self, tab: MarketplaceTab) -> Vec<MarketplaceItem> {
        match tab {
            MarketplaceTab::Apps => builtin_apps_catalog(),
            MarketplaceTab::Agents => builtin_agents_catalog(),
            MarketplaceTab::Skills => {
                let mut items = builtin_skills_catalog();
                let mut seen: HashSet<String> = items
                    .iter()
                    .flat_map(|i| [i.id.to_lowercase(), i.slug.to_lowercase()])
                    .collect();
                for local in load_local_installed_skills(&self.data_dir) {
                    if seen.insert(local.slug.to_lowercase()) {
                        seen.insert(local.id.to_lowercase());
                        items.push(local);
                    }
                }
                items
            }
        }
    }
}

fn install_key(tab: MarketplaceTab, id: &str) -> String {
    format!("{}:{id}", tab.as_str())
}

fn item_matches_query(item: &MarketplaceItem, q_lower: &str) -> bool {
    item.name.to_lowercase().contains(q_lower)
        || item.slug.to_lowercase().contains(q_lower)
        || item.id.to_lowercase().contains(q_lower)
        || item.tagline.to_lowercase().contains(q_lower)
        || item.description.to_lowercase().contains(q_lower)
        || item.creator_name.to_lowercase().contains(q_lower)
        || item.category.to_lowercase().contains(q_lower)
}

fn load_installs_file(path: &Path) -> RuntimeResult<MarketplaceInstallsFile> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            if raw.trim().is_empty() {
                return Ok(MarketplaceInstallsFile {
                    version: 1,
                    installs: BTreeMap::new(),
                });
            }
            let parsed: MarketplaceInstallsFile = serde_json::from_str(&raw)?;
            Ok(parsed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(MarketplaceInstallsFile {
            version: 1,
            installs: BTreeMap::new(),
        }),
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

fn load_local_installed_skills(data_dir: &Path) -> Vec<MarketplaceItem> {
    let path = data_dir.join(INSTALLED_SKILLS_FILE_NAME);
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<LocalInstalledSkillsFile>(&raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen_slugs = HashSet::new();
    for entries in parsed.by_agent.values() {
        for entry in entries {
            let slug = entry.slug.trim();
            if slug.is_empty() || !seen_slugs.insert(slug.to_lowercase()) {
                continue;
            }
            out.push(MarketplaceItem {
                id: entry.skill_id.clone(),
                slug: slug.to_string(),
                tab: MarketplaceTab::Skills,
                name: entry.name.clone(),
                tagline: "Locally installed SKILL.md skill".to_string(),
                description: entry.description.clone(),
                category: "other".to_string(),
                creator_name: "Local Workspace".to_string(),
                verified: false,
                featured: false,
                installs: 1,
                kind: "local-skill".to_string(),
            });
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn make_item(
    id: &str,
    slug: &str,
    tab: MarketplaceTab,
    name: &str,
    tagline: &str,
    description: &str,
    category: &str,
    creator_name: &str,
    verified: bool,
    featured: bool,
    installs: u64,
    kind: &str,
) -> MarketplaceItem {
    MarketplaceItem {
        id: id.to_string(),
        slug: slug.to_string(),
        tab,
        name: name.to_string(),
        tagline: tagline.to_string(),
        description: description.to_string(),
        category: category.to_string(),
        creator_name: creator_name.to_string(),
        verified,
        featured,
        installs,
        kind: kind.to_string(),
    }
}

fn builtin_apps_catalog() -> Vec<MarketplaceItem> {
    let t = MarketplaceTab::Apps;
    vec![
        // 8 Native OpenBot Connectors
        make_item("connector:slack", "slack", t, "Slack", "Bridge channels and threads with Slack workspaces", "Connect Slack workspaces for bidirectional agent messaging, mentions, and workflow triggers.", "automation", "OpenBot", true, true, 18_420, "connector"),
        make_item("connector:discord", "discord", t, "Discord", "Connect Discord servers with role-based access control", "Link Discord guilds and channels to OpenBot agents with per-role ACLs and thread replies.", "automation", "OpenBot", true, true, 16_910, "connector"),
        make_item("connector:telegram", "telegram", t, "Telegram", "Bidirectional Telegram chat and channel bridge", "Connect Telegram bots and groups directly to OpenBot agents and workflows.", "automation", "OpenBot", true, true, 22_350, "connector"),
        make_item("connector:whatsapp", "whatsapp", t, "WhatsApp", "Private WhatsApp bridge with pairing and sender rules", "Pair WhatsApp accounts with sender allowlists, self-chat support, and history-only contacts.", "automation", "OpenBot", true, true, 14_780, "connector"),
        make_item("connector:github", "github", t, "GitHub", "Repositories, pull requests, issues, and workflow triggers", "Connect GitHub accounts to triage issues, review pull requests, and trigger workflows on events.", "coding", "OpenBot", true, true, 29_140, "connector"),
        make_item("connector:onepassword", "onepassword", t, "1Password", "Zero-exposure secret autofill from 1Password vaults", "Supply credentials and passcodes to agent browser runs without revealing raw secrets to models.", "productivity", "OpenBot", true, false, 9_840, "connector"),
        make_item("connector:bitwarden", "bitwarden", t, "Bitwarden", "Open-source password vault integration", "Authenticate logins and TOTP codes securely through your Bitwarden vault.", "productivity", "OpenBot", true, false, 7_620, "connector"),
        make_item("connector:protonpass", "protonpass", t, "Proton Pass", "End-to-end encrypted Proton Pass vault connector", "Connect Proton Pass vaults for masked credential submission inside browser automation.", "productivity", "OpenBot", true, false, 5_430, "connector"),
        // Verified MCP Apps
        make_item("app:linear", "linear", t, "Linear", "Issue tracking, sprint planning, and roadmap triage", "Create, search, and update Linear issues, cycles, and projects from any agent thread.", "productivity", "Linear", true, true, 15_200, "plugin"),
        make_item("app:notion", "notion", t, "Notion", "Search and edit Notion docs, wikis, and databases", "Read workspace pages, append notes, and query Notion databases via MCP.", "documents", "Notion", true, true, 19_600, "plugin"),
        make_item("app:figma", "figma", t, "Figma", "Inspect design frames, tokens, and component specs", "Pull component hierarchies, design variables, and comments directly from Figma files.", "design", "Figma", true, true, 11_850, "plugin"),
        make_item("app:canva", "canva", t, "Canva", "Create and export visual assets and presentations", "Generate, edit, and export Canva templates and brand graphics.", "design", "Canva", true, false, 8_420, "plugin"),
        make_item("app:stripe", "stripe", t, "Stripe", "Read-only billing, subscriptions, and invoice telemetry", "Inspect Stripe customers, subscriptions, and invoices with restricted read-only API keys.", "data-analytics", "Stripe", true, true, 13_410, "plugin"),
        make_item("app:vercel", "vercel", t, "Vercel", "Deployments, preview logs, and project domains", "Monitor Vercel builds, inspect runtime logs, and manage preview deployments.", "coding", "Vercel", true, true, 12_730, "plugin"),
        make_item("app:cloudflare", "cloudflare", t, "Cloudflare", "Workers, KV, R2, DNS, and edge analytics", "Inspect Cloudflare Workers deployments, zone records, and traffic analytics.", "coding", "Cloudflare", true, false, 10_190, "plugin"),
        make_item("app:supabase", "supabase", t, "Supabase", "Postgres schemas, edge functions, and auth inspection", "Query Supabase projects, inspect migrations, and monitor database health.", "data-analytics", "Supabase", true, true, 14_050, "plugin"),
        make_item("app:sentry", "sentry", t, "Sentry", "Error monitoring, stack traces, and release regressions", "Investigate Sentry issues, inspect breadcrumbs, and correlate production regressions.", "coding", "Sentry", true, false, 9_310, "plugin"),
        make_item("app:brave-search", "brave-search", t, "Brave Search", "Fast web and news search API for agent research", "Run live web, news, and documentation queries with clean structured snippets.", "research", "Brave", true, true, 17_240, "plugin"),
        make_item("app:obsidian", "obsidian", t, "Obsidian", "Local Markdown knowledge base search and capture", "Read, link, and organize notes inside local Obsidian vaults.", "documents", "Obsidian", true, false, 11_120, "plugin"),
        make_item("app:atlassian", "atlassian", t, "Atlassian", "Jira issues and Confluence knowledge spaces", "Search Jira boards, update sprint tickets, and summarize Confluence pages.", "productivity", "Atlassian", true, false, 10_680, "plugin"),
        make_item("app:airtable", "airtable", t, "Airtable", "Structured bases, views, and operational tables", "Query and update Airtable bases, records, and field schemas.", "data-analytics", "Airtable", true, false, 7_890, "plugin"),
        make_item("app:posthog", "posthog", t, "PostHog", "Product analytics, funnels, and feature flags", "Query PostHog insights, retention cohorts, and experiment flag states.", "data-analytics", "PostHog", true, false, 8_760, "plugin"),
        make_item("app:zapier", "zapier", t, "Zapier", "Trigger multi-app automations across thousands of SaaS tools", "Invoke Zapier actions and webhook zaps from agent routines and workflows.", "automation", "Zapier", true, false, 12_150, "plugin"),
        // Public Community MCP Apps
        make_item("app:postgres", "postgres", t, "PostgreSQL Inspector", "Read-only SQL introspection for Postgres databases", "Community MCP connector for schema exploration and explain-plan diagnostics.", "data-analytics", "Community Contributors", false, false, 4_280, "plugin"),
        make_item("app:aave", "aave", t, "Aave Protocol Monitor", "DeFi liquidity pool rates and health-factor tracking", "Public community MCP server for inspecting Aave V3 markets, APYs, and positions.", "research", "DeFi Open Tools", false, false, 2_190, "plugin"),
    ]
}

fn builtin_agents_catalog() -> Vec<MarketplaceItem> {
    let t = MarketplaceTab::Agents;
    vec![
        make_item("agent:research-analyst", "research-analyst", t, "Research Analyst", "Deep synthesis, source verification, and structured briefs", "Investigates technical, market, and academic topics with cited multi-source reports.", "research", "OpenBot", true, true, 14_320, "agent"),
        make_item("agent:code-reviewer", "code-reviewer", t, "Code Reviewer", "Thorough PR audits for correctness, security, and edge cases", "Reviews diffs across Rust, TypeScript, and Python repositories with actionable feedback.", "coding", "OpenBot", true, true, 19_480, "agent"),
        make_item("agent:support-concierge", "support-concierge", t, "Support Concierge", "Customer triage, CRM context lookup, and draft responses", "Reads redacted CRM snapshots, drafts empathetic replies, and proposes follow-up updates.", "productivity", "OpenBot", true, true, 11_260, "agent"),
        make_item("agent:release-manager", "release-manager", t, "Release Manager", "Changelog generation, QA checklists, and rollout coordination", "Coordinates multi-bot channel releases, tracks CI status, and prepares deployment summaries.", "automation", "OpenBot", true, false, 9_750, "agent"),
        make_item("agent:data-steward", "data-steward", t, "Data Steward", "SQL rollups, ARR metrics, and anomaly summaries", "Builds reproducible SQLite and warehouse queries for revenue operations and KPI dashboards.", "data-analytics", "OpenBot", true, false, 8_610, "agent"),
        make_item("agent:design-critic", "design-critic", t, "Design Critic", "UI polish, layout spacing, and accessibility reviews", "Audits interactive HTML previews and Figma frames for visual hierarchy and contrast.", "design", "OpenBot", true, false, 7_140, "agent"),
        make_item("agent:community-ops", "community-ops", t, "Community Ops Bot", "Digest threads across Discord, Slack, and Telegram", "Public community template for summarizing daily channel discussions and unanswered questions.", "automation", "Community Makers", false, false, 4_520, "agent"),
        make_item("agent:indie-marketer", "indie-marketer", t, "Indie Launch Writer", "Release posts, landing page copy, and newsletter drafts", "Public agent template tailored for solo builders launching static sites and product updates.", "documents", "Indie Builders", false, false, 3_890, "agent"),
    ]
}

fn builtin_skills_catalog() -> Vec<MarketplaceItem> {
    let t = MarketplaceTab::Skills;
    vec![
        make_item("skill:daily-briefing", "daily-briefing", t, "Daily Executive Briefing", "Compile morning priorities, open waits, and channel digests", "SKILL.md workflow that aggregates unread channels, routine results, and calendar items.", "productivity", "OpenBot", true, true, 16_100, "skill"),
        make_item("skill:pdf-synthesis", "pdf-synthesis", t, "PDF & Spec Synthesis", "Extract tables, requirements, and structured summaries from documents", "SKILL.md instruction pack for accurate long-document outlining and citation indexing.", "documents", "OpenBot", true, true, 13_840, "skill"),
        make_item("skill:ui-design-critique", "ui-design-critique", t, "UI Design Critique", "Inspect typography, contrast, rhythm, and responsive breakpoints", "SKILL.md checklist for reviewing HTML previews, dark/light palettes, and spacing.", "design", "OpenBot", true, true, 10_920, "skill"),
        make_item("skill:deep-literature-review", "deep-literature-review", t, "Deep Literature Review", "Cross-check claims across primary sources with inline references", "SKILL.md methodology for structured research briefs and conflicting-evidence tables.", "research", "OpenBot", true, true, 12_470, "skill"),
        make_item("skill:sql-cohort-analysis", "sql-cohort-analysis", t, "SQL Cohort & ARR Analysis", "Build reproducible retention, MRR, and funnel rollups in SQLite", "SKILL.md query patterns for agent-data.db analytics and interactive HTML charts.", "data-analytics", "OpenBot", true, true, 11_350, "skill"),
        make_item("skill:ci-triage-automation", "ci-triage-automation", t, "CI Failure Triage", "Classify flaky vs deterministic build failures and open fixes", "SKILL.md playbook for webhook-triggered routine runs and GitHub workflow checks.", "automation", "OpenBot", true, true, 14_790, "skill"),
        make_item("skill:rust-refactor-coach", "rust-refactor-coach", t, "Rust Refactor Coach", "Idiomatic error handling, zero-copy parsing, and modular crate design", "SKILL.md coding guide for strict zero-warning Rust systems engineering.", "coding", "OpenBot", true, true, 15_630, "skill"),
        make_item("skill:meeting-notes-cleaner", "meeting-notes-cleaner", t, "Meeting Notes Cleaner", "Turn raw transcripts into owners, decisions, and action items", "Community SKILL.md skill for structuring voice transcripts and standup notes.", "other", "Community Makers", false, false, 5_180, "skill"),
        make_item("skill:changelog-scribe", "changelog-scribe", t, "Changelog Scribe", "Generate user-facing release notes from git commits and PRs", "Public SKILL.md template grouping commits into highlights, fixes, and breaking changes.", "documents", "OSS Collective", false, false, 4_640, "skill"),
        make_item("skill:competitor-radar", "competitor-radar", t, "Competitor Radar", "Track public changelogs, pricing pages, and launch announcements", "Public SKILL.md skill for scheduled weekly product intelligence updates.", "research", "Growth Lab", false, false, 3_920, "skill"),
    ]
}
