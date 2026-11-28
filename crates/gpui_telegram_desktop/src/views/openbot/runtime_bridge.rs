//! Live [`OpenBotHostRuntime`] bridge & bidirectional Telegram <-> OpenBot peer projection (`OB-F20`).
//!
//! Wraps `Arc<OpenBotHostRuntime>` opened at `<data_dir>` to hydrate [`OpenBotWorkspaceShell`]
//! with live SQLite-backed agents, multi-bot shared channels, channel messages/tasks/waits,
//! workflows, routines, and CRM overview records, and projects active OpenBot agents and shared
//! channels into `gpui-telegram`'s native `ChatSidebar` and `@mention` autocomplete candidates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openbot_contracts::agents::AgentSummary;
use openbot_contracts::avatar::{bloub_avatar_profile, AvatarHue};
use openbot_contracts::channels::{ChannelSummary, ChannelTask, SIGNED_OUT_CHANNEL_MEMBER_ID};
use openbot_contracts::crm::CrmPaymentState;
use openbot_contracts::routines::RoutineSchedule;
use openbot_runtime::OpenBotHostRuntime;
use openbot_storage::{
    AgentStore, ChannelStore, CrmCustomerPageInput, CrmPaymentPageInput, CrmStore, HostAnalytics,
    HostAnalyticsInput, OpenBotDatabase,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::channel_view::{
    ChannelConversationState, ChannelStoppedTasksState, ChannelTaskWaitNotice,
    ChannelTimelineMessage,
};
use super::secondary_pages::{RoutineCalendarItem, RoutineStatusPill};
use super::sidebar::{SidebarAgentItem, SidebarChannelItem};
use super::ui_probe::LiveFusionSnapshot;
use super::workspace_shell::{ActiveOpenBotTarget, OpenBotWorkspaceShell};

/// Reserved high-bit namespace prefix (`"OB"`) for synthetic Telegram dialog IDs projected
/// from live OpenBot agents and shared channels so they never collide with real Telegram IDs.
pub const OPENBOT_SYNTHETIC_DIALOG_PREFIX: u64 = 0x4F42_0000_0000_0000;

/// Deterministically derives a non-colliding synthetic Telegram dialog/user ID for an OpenBot
/// agent or multi-bot shared channel ID.
pub fn synthetic_telegram_peer_id(namespace: &str, openbot_id: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(b":");
    hasher.update(openbot_id.as_bytes());
    let digest = hasher.finalize();
    let low48 = u64::from_be_bytes([
        0, 0, digest[0], digest[1], digest[2], digest[3], digest[4], digest[5],
    ]);
    OPENBOT_SYNTHETIC_DIALOG_PREFIX | (low48.max(1))
}

/// Converts a `#RRGGBB` CSS color string into a packed 24-bit `0xRRGGBB` RGB integer.
pub fn parse_hex_rgb(hex: &str, fallback: u32) -> u32 {
    let trimmed = hex.trim().trim_start_matches('#');
    if trimmed.len() == 6 {
        u32::from_str_radix(trimmed, 16).unwrap_or(fallback)
    } else {
        fallback
    }
}

/// Builds a clean Telegram `@mention` handle slug from an OpenBot agent or channel display name.
pub fn sanitize_mention_handle(name: &str, fallback_id: &str) -> String {
    let mut handle = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            handle.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || ch == '-' {
            if !handle.ends_with('_') && !handle.is_empty() {
                handle.push('_');
            }
        }
    }
    let trimmed = handle.trim_matches('_').to_string();
    if trimmed.is_empty() {
        let mut id_slug: String = fallback_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if id_slug.is_empty() {
            id_slug.push_str("openbot");
        }
        id_slug
    } else {
        trimmed
    }
}

/// Computes compact 1–2 character initials for a Telegram dialog avatar fallback.
pub fn initials_for_label(label: &str, fallback: &str) -> String {
    let mut chars = label
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .take(2);
    let mut out = String::new();
    if let Some(c1) = chars.next() {
        out.push(c1);
    }
    if let Some(c2) = chars.next() {
        out.push(c2);
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// Resolved OpenBot `plush:<seed>` / `bloub:<seed>` avatar descriptor projected into Telegram UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedTelegramAvatarProfile {
    pub entity_id: String,
    pub display_name: String,
    pub is_channel: bool,
    pub avatar_spec: String,
    pub resolved_shape: String,
    pub resolved_color_hex: String,
    pub avatar_url: Option<String>,
}

/// Live OpenBot agent or multi-bot channel projected as a Telegram peer entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedTelegramPeer {
    pub synthetic_dialog_id: u64,
    pub openbot_id: String,
    pub display_name: String,
    pub subtitle: String,
    pub avatar_initials: String,
    pub avatar_spec: String,
    pub avatar_color_hex: String,
    pub avatar_color_rgb: u32,
    pub is_channel: bool,
    pub member_count: usize,
    pub mention_handle: String,
    pub unread_count: u32,
}

/// Complete live bidirectional Telegram <-> OpenBot projection snapshot (`OB-F20`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramBidirectionalProjection {
    pub projected_agent_peers: Vec<ProjectedTelegramPeer>,
    pub projected_channel_peers: Vec<ProjectedTelegramPeer>,
    pub mention_candidates: Vec<String>,
    pub avatar_profiles: Vec<ProjectedTelegramAvatarProfile>,
}

impl TelegramBidirectionalProjection {
    /// Builds a projection from live non-deleted OpenBot agents and channel summaries.
    pub fn from_agents_and_channels(
        agents: &[AgentSummary],
        channels: &[ChannelSummary],
    ) -> Self {
        let mut projected_agent_peers = Vec::with_capacity(agents.len());
        let mut projected_channel_peers = Vec::new();
        let mut mention_candidates = Vec::with_capacity(agents.len());
        let mut seen_mentions = HashSet::new();
        let mut avatar_profiles = Vec::with_capacity(agents.len() + channels.len());

        for agent in agents {
            let seed = if agent.avatar_seed.trim().is_empty() {
                agent.id.as_str()
            } else {
                agent.avatar_seed.as_str()
            };
            let explicit_color = agent
                .avatar_color
                .as_ref()
                .and_then(|opt| opt.as_deref());
            let profile = bloub_avatar_profile(seed, agent.avatar_hue, explicit_color);
            let avatar_spec = format!("plush:{seed}");
            let color_rgb = parse_hex_rgb(&profile.color, 0x007CF7);
            let subtitle = if !agent.preview.trim().is_empty() {
                agent.preview.clone()
            } else if !agent.title.trim().is_empty() {
                agent.title.clone()
            } else {
                "OpenBot Agent".to_string()
            };
            let handle = sanitize_mention_handle(&agent.name, &agent.id);
            let dialog_id = synthetic_telegram_peer_id("openbot-agent", &agent.id);

            projected_agent_peers.push(ProjectedTelegramPeer {
                synthetic_dialog_id: dialog_id,
                openbot_id: agent.id.clone(),
                display_name: agent.name.clone(),
                subtitle,
                avatar_initials: initials_for_label(&agent.name, "OB"),
                avatar_spec: avatar_spec.clone(),
                avatar_color_hex: profile.color.clone(),
                avatar_color_rgb: color_rgb,
                is_channel: false,
                member_count: 1,
                mention_handle: handle,
                unread_count: 0,
            });
            avatar_profiles.push(ProjectedTelegramAvatarProfile {
                entity_id: agent.id.clone(),
                display_name: agent.name.clone(),
                is_channel: false,
                avatar_spec,
                resolved_shape: profile.shape.as_str().to_string(),
                resolved_color_hex: profile.color,
                avatar_url: agent.avatar_url.clone(),
            });
            let trimmed_name = agent.name.trim();
            if !trimmed_name.is_empty() && seen_mentions.insert(trimmed_name.to_ascii_lowercase()) {
                mention_candidates.push(trimmed_name.to_string());
            }
        }

        for summary in channels {
            if summary.channel.archived {
                continue;
            }
            let ch = &summary.channel;
            let hue = ch
                .draft
                .icon
                .as_ref()
                .and_then(|opt| opt.as_ref())
                .and_then(|i| AvatarHue::from_degrees(i.hue));
            let profile = bloub_avatar_profile(&ch.id, hue, None);
            let avatar_spec = format!("bloub:{}", ch.id);
            let color_rgb = parse_hex_rgb(&profile.color, 0x007CF7);
            let subtitle = summary
                .last_message
                .as_ref()
                .map(|m| format!("{}: {}", m.author_name, m.text))
                .unwrap_or_else(|| {
                    format!(
                        "OpenBot Channel · {} agents",
                        ch.draft.members.len()
                    )
                });
            let handle = sanitize_mention_handle(&ch.draft.name, &ch.id);
            let dialog_id = synthetic_telegram_peer_id("openbot-channel", &ch.id);
            let image_url = ch.image_url.as_ref().and_then(|opt| opt.clone());

            projected_channel_peers.push(ProjectedTelegramPeer {
                synthetic_dialog_id: dialog_id,
                openbot_id: ch.id.clone(),
                display_name: ch.draft.name.clone(),
                subtitle,
                avatar_initials: initials_for_label(&ch.draft.name, "CH"),
                avatar_spec: avatar_spec.clone(),
                avatar_color_hex: profile.color.clone(),
                avatar_color_rgb: color_rgb,
                is_channel: true,
                member_count: ch.draft.members.len(),
                mention_handle: handle,
                unread_count: summary.unread_count.min(u64::from(u32::MAX)) as u32,
            });
            avatar_profiles.push(ProjectedTelegramAvatarProfile {
                entity_id: ch.id.clone(),
                display_name: ch.draft.name.clone(),
                is_channel: true,
                avatar_spec,
                resolved_shape: profile.shape.as_str().to_string(),
                resolved_color_hex: profile.color,
                avatar_url: image_url,
            });
        }

        Self {
            projected_agent_peers,
            projected_channel_peers,
            mention_candidates,
            avatar_profiles,
        }
    }

    /// Converts this live Telegram projection + per-channel member map into the
    /// [`LiveFusionSnapshot`] consumed by `evaluate_ui_probe`.
    pub fn to_live_fusion_snapshot(
        &self,
        channel_members_by_id: HashMap<String, Vec<String>>,
    ) -> LiveFusionSnapshot {
        LiveFusionSnapshot {
            projected_agent_peers: self.projected_agent_peers.len(),
            projected_channel_peers: self.projected_channel_peers.len(),
            mention_candidates: self.mention_candidates.clone(),
            channel_members_by_id,
        }
    }
}

/// Reads live non-deleted agents, non-archived channels, `@mention` candidates, and per-channel
/// member lists directly from `<data_dir>/openbot.db`.
///
/// Returns an empty [`LiveFusionSnapshot`] gracefully if `openbot.db` has not been created yet.
pub fn load_live_fusion_snapshot(data_dir: &Path) -> Result<LiveFusionSnapshot, String> {
    let db_path = data_dir.join("openbot.db");
    if !db_path.is_file() {
        return Ok(LiveFusionSnapshot::default());
    }
    let db = OpenBotDatabase::open(&db_path)
        .map_err(|err| format!("failed to open {}: {err}", db_path.display()))?;
    let agents = AgentStore::open(db.clone(), data_dir)
        .map_err(|err| format!("failed to open openbot agents store: {err}"))?
        .list();
    let channels = ChannelStore::new(db)
        .list(SIGNED_OUT_CHANNEL_MEMBER_ID, true)
        .map_err(|err| format!("failed to read openbot channels: {err}"))?;

    let mut channel_members_by_id: HashMap<String, Vec<String>> = HashMap::new();
    for ch in &channels {
        let member_ids: Vec<String> = ch
            .channel
            .draft
            .members
            .iter()
            .map(|m| m.agent_id.clone())
            .collect();
        channel_members_by_id.insert(ch.channel.id.clone(), member_ids);
    }

    let projection = TelegramBidirectionalProjection::from_agents_and_channels(&agents, &channels);
    Ok(projection.to_live_fusion_snapshot(channel_members_by_id))
}

/// Loads a full [`TelegramBidirectionalProjection`] from `<data_dir>/openbot.db` if present.
pub fn load_telegram_bidirectional_projection(
    data_dir: &Path,
) -> Result<TelegramBidirectionalProjection, String> {
    let db_path = data_dir.join("openbot.db");
    if !db_path.is_file() {
        return Ok(TelegramBidirectionalProjection::default());
    }
    let db = OpenBotDatabase::open(&db_path)
        .map_err(|err| format!("failed to open {}: {err}", db_path.display()))?;
    let agents = AgentStore::open(db.clone(), data_dir)
        .map_err(|err| format!("failed to open openbot agents store: {err}"))?
        .list();
    let channels = ChannelStore::new(db)
        .list(SIGNED_OUT_CHANNEL_MEMBER_ID, true)
        .map_err(|err| format!("failed to read openbot channels: {err}"))?;
    Ok(TelegramBidirectionalProjection::from_agents_and_channels(
        &agents, &channels,
    ))
}

fn summarize_routine_schedule(schedule: &RoutineSchedule) -> String {
    match schedule {
        RoutineSchedule::Hourly { minute } => format!("Hourly at :{minute:02}"),
        RoutineSchedule::Daily { time } => format!("Daily at {time}"),
        RoutineSchedule::Weekdays { time } => format!("Weekdays at {time}"),
        RoutineSchedule::Weekly { weekday, time } => format!("Weekly (d{weekday}) at {time}"),
        RoutineSchedule::Monthly { day, time } => format!("Monthly (day {day}) at {time}"),
        RoutineSchedule::Interval { amount, .. } => format!("Every {amount}"),
        RoutineSchedule::Advanced { .. } => "Advanced schedule".to_string(),
        RoutineSchedule::Custom { expression } => format!("Cron {expression}"),
        RoutineSchedule::Once { date, time } => format!("Once {date} {time}"),
    }
}

fn split_iso_date_and_hhmm(iso: &str) -> (String, String) {
    let trimmed = iso.trim();
    if trimmed.len() >= 16 && trimmed.as_bytes().get(10) == Some(&b'T') {
        (trimmed[0..10].to_string(), trimmed[11..16].to_string())
    } else if trimmed.len() >= 10 {
        (trimmed[0..10].to_string(), "09:00".to_string())
    } else {
        ("2026-03-29".to_string(), "09:00".to_string())
    }
}

/// Live desktop runtime adapter bridging `Arc<OpenBotHostRuntime>` into [`OpenBotWorkspaceShell`]
/// and `gpui-telegram`'s native sidebar/composer peer projections.
pub struct OpenBotRuntimeBridge {
    runtime: Arc<OpenBotHostRuntime>,
    data_dir: PathBuf,
}

impl OpenBotRuntimeBridge {
    /// Opens a live [`OpenBotHostRuntime`] rooted at `data_dir` and wraps it in a bridge.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = data_dir.as_ref().to_path_buf();
        let runtime = OpenBotHostRuntime::open(&dir)
            .map_err(|err| format!("failed to open OpenBotHostRuntime at {}: {err}", dir.display()))?;
        Ok(Self {
            runtime,
            data_dir: dir,
        })
    }

    /// Wraps an already-opened [`OpenBotHostRuntime`] handle.
    pub fn from_runtime(runtime: Arc<OpenBotHostRuntime>) -> Self {
        let data_dir = runtime.data_dir().to_path_buf();
        Self { runtime, data_dir }
    }

    pub fn runtime(&self) -> &Arc<OpenBotHostRuntime> {
        &self.runtime
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Loads live non-deleted OpenBot agents formatted as [`SidebarAgentItem`] rows.
    pub fn load_sidebar_agents(&self) -> Vec<SidebarAgentItem> {
        self.runtime
            .agents()
            .list_agents()
            .iter()
            .map(SidebarAgentItem::from_agent_summary)
            .collect()
    }

    /// Loads live OpenBot multi-bot channels formatted as [`SidebarChannelItem`] rows.
    pub fn load_sidebar_channels(&self) -> Result<Vec<SidebarChannelItem>, String> {
        let summaries = self
            .runtime
            .channels()
            .list(None, true)
            .map_err(|err| format!("failed to list OpenBot channels: {err}"))?;
        Ok(summaries
            .iter()
            .map(SidebarChannelItem::from_channel_summary)
            .collect())
    }

    /// Computes a live [`TelegramBidirectionalProjection`] from the runtime's SQLite stores.
    pub fn build_telegram_projection(&self) -> Result<TelegramBidirectionalProjection, String> {
        let agents = self.runtime.agents().list_agents();
        let channels = self
            .runtime
            .channels()
            .list(None, true)
            .map_err(|err| format!("failed to list OpenBot channels: {err}"))?;
        Ok(TelegramBidirectionalProjection::from_agents_and_channels(
            &agents, &channels,
        ))
    }

    /// Computes a live [`LiveFusionSnapshot`] backed directly by the runtime's stores.
    pub fn live_fusion_snapshot(&self) -> Result<LiveFusionSnapshot, String> {
        let agents = self.runtime.agents().list_agents();
        let channels = self
            .runtime
            .channels()
            .list(None, true)
            .map_err(|err| format!("failed to list OpenBot channels: {err}"))?;
        let mut channel_members_by_id: HashMap<String, Vec<String>> = HashMap::new();
        for ch in &channels {
            let member_ids: Vec<String> = ch
                .channel
                .draft
                .members
                .iter()
                .map(|m| m.agent_id.clone())
                .collect();
            channel_members_by_id.insert(ch.channel.id.clone(), member_ids);
        }
        let projection =
            TelegramBidirectionalProjection::from_agents_and_channels(&agents, &channels);
        Ok(projection.to_live_fusion_snapshot(channel_members_by_id))
    }

    /// Hydrates a [`ChannelConversationState`] with live messages, stopped tasks, and waits
    /// from [`openbot_runtime::ChannelService::get_detail`].
    pub fn hydrate_channel_conversation(
        &self,
        channel_id: &str,
        agent_names: &BTreeMap<String, String>,
    ) -> Result<ChannelConversationState, String> {
        let detail = self
            .runtime
            .channels()
            .get_detail(channel_id)
            .map_err(|err| format!("failed to load channel '{channel_id}': {err}"))?;
        let members: Vec<String> = detail
            .channel
            .draft
            .members
            .iter()
            .map(|m| m.agent_id.clone())
            .collect();
        let lead_id = detail
            .channel
            .draft
            .lead_agent_id
            .clone()
            .or_else(|| members.first().cloned());

        let mut conv = ChannelConversationState::new(
            detail.channel.id.clone(),
            detail.channel.draft.name.clone(),
            members,
            lead_id,
        );
        conv.title = detail.channel.draft.title.clone();
        conv.instructions = detail.channel.draft.instructions.clone();
        conv.icon = detail.channel.draft.icon.clone().flatten();
        conv.image_url = detail.channel.image_url.clone().flatten();
        conv.isolated = detail.channel.draft.isolated.unwrap_or(false);
        conv.set_archived(detail.channel.archived);
        conv.agent_names = agent_names.clone();

        let msg_text_by_id: HashMap<String, String> = detail
            .messages
            .iter()
            .map(|m| (m.id.clone(), m.message.text.clone()))
            .collect();
        conv.messages = detail
            .messages
            .iter()
            .map(|m| {
                let reply_preview = m
                    .message
                    .reply_to_message_id
                    .as_ref()
                    .and_then(|opt| opt.as_ref())
                    .and_then(|rid| msg_text_by_id.get(rid))
                    .cloned();
                ChannelTimelineMessage::from_channel_message(m, reply_preview)
            })
            .collect();

        let contract_tasks: Vec<ChannelTask> = detail.tasks.iter().map(|tv| tv.task.clone()).collect();
        conv.stopped_tasks = ChannelStoppedTasksState::from_channel_tasks(&contract_tasks);
        conv.wait_notices = detail
            .waits
            .iter()
            .map(|w| {
                if w.reason == "agent-busy" {
                    ChannelTaskWaitNotice::AgentBusy {
                        task_id: w.task_id.clone(),
                        agent_id: w.agent_id.clone(),
                        channel_id: w.channel_id.clone().unwrap_or_default(),
                        channel_name: w.channel_name.clone().unwrap_or_default(),
                    }
                } else {
                    ChannelTaskWaitNotice::ChannelFull {
                        task_id: w.task_id.clone(),
                        agent_id: w.agent_id.clone(),
                        parallel_limit: super::channel_view::CHANNEL_PARALLEL_LIMIT,
                    }
                }
            })
            .collect();
        Ok(conv)
    }

    /// Hydrates an [`OpenBotWorkspaceShell`] end-to-end from live SQLite stores:
    /// agents, channels, active channel messages/tasks, workflows, routines, and CRM overview.
    pub fn hydrate_workspace_shell(
        &self,
        shell: &mut OpenBotWorkspaceShell,
    ) -> Result<TelegramBidirectionalProjection, String> {
        shell.data_dir = Some(self.data_dir.clone());
        shell.shell.load_from_data_dir(&self.data_dir);

        let raw_agents = self.runtime.agents().list_agents();
        let raw_channels = self
            .runtime
            .channels()
            .list(None, true)
            .map_err(|err| format!("failed to hydrate channels: {err}"))?;

        shell.agents = raw_agents
            .iter()
            .map(SidebarAgentItem::from_agent_summary)
            .collect();
        shell.channels = raw_channels
            .iter()
            .map(SidebarChannelItem::from_channel_summary)
            .collect();
        shell.composer.available_agents = shell
            .agents
            .iter()
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect();

        let agent_names: BTreeMap<String, String> = shell
            .agents
            .iter()
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect();

        let channel_to_hydrate: Option<String> = match &shell.active_target {
            ActiveOpenBotTarget::Channel(cid) => Some(cid.clone()),
            _ => raw_channels
                .iter()
                .find(|c| !c.channel.archived)
                .map(|c| c.channel.id.clone()),
        };
        if let Some(cid) = channel_to_hydrate {
            if let Ok(conv) = self.hydrate_channel_conversation(&cid, &agent_names) {
                shell.channel_conversation = conv;
            }
        }

        shell.set_workflow_runtime(std::sync::Arc::clone(&self.runtime));
        shell.sync_workflow_agents(
            shell
                .agents
                .iter()
                .map(|a| (a.id.clone(), a.name.clone()))
                .collect(),
        );
        super::workflow_interactive::hydrate_workflows_from_runtime(
            &mut shell.workflow_canvas,
            &self.runtime,
            &self.data_dir,
        );

        let mut calendar_items: Vec<RoutineCalendarItem> = Vec::new();
        for agent in &raw_agents {
            if let Ok(routines) = self.runtime.routines().list_routines(&agent.id, false) {
                for r in routines {
                    let (date_ymd, time_hhmm) = split_iso_date_and_hhmm(&r.trigger.next_run_at);
                    calendar_items.push(RoutineCalendarItem {
                        id: r.id,
                        name: r.name,
                        agent_id: r.owner_id,
                        date_ymd,
                        time_hhmm,
                        timezone: r.timezone,
                        status: if r.active {
                            RoutineStatusPill::Active
                        } else {
                            RoutineStatusPill::Paused
                        },
                        schedule_summary: summarize_routine_schedule(&r.trigger.schedule),
                        is_webhook: false,
                    });
                }
            }
        }
        for ch in &raw_channels {
            if let Ok(routines) = self.runtime.routines().list_routines(&ch.channel.id, true) {
                for r in routines {
                    let (date_ymd, time_hhmm) = split_iso_date_and_hhmm(&r.trigger.next_run_at);
                    calendar_items.push(RoutineCalendarItem {
                        id: r.id,
                        name: r.name,
                        agent_id: r.owner_id,
                        date_ymd,
                        time_hhmm,
                        timezone: r.timezone,
                        status: if r.active {
                            RoutineStatusPill::Active
                        } else {
                            RoutineStatusPill::Paused
                        },
                        schedule_summary: summarize_routine_schedule(&r.trigger.schedule),
                        is_webhook: false,
                    });
                }
            }
        }
        if !calendar_items.is_empty() {
            shell.routine_calendar.items = calendar_items;
        }

        let crm_store = CrmStore::new(self.runtime.database().clone());
        if let Ok(overview) = crm_store.overview() {
            shell.crm_page.overview = overview;
        }
        if let Ok(cust_page) = crm_store.customers(&CrmCustomerPageInput { after: None }) {
            shell.crm_page.customers = cust_page.customers;
        }
        let mut live_payments = Vec::new();
        if let Ok(open_page) = crm_store.payments(&CrmPaymentPageInput {
            state: CrmPaymentState::Open,
            customer_id: None,
            after: None,
        }) {
            live_payments.extend(open_page.payments);
        }
        if let Ok(paid_page) = crm_store.payments(&CrmPaymentPageInput {
            state: CrmPaymentState::Paid,
            customer_id: None,
            after: None,
        }) {
            live_payments.extend(paid_page.payments);
        }
        if !live_payments.is_empty() {
            shell.crm_page.payments = live_payments;
        }

        let range_days = shell.usage_credits.selected_range_days.clamp(1, 90);
        if let Some(analytics) = self.read_host_usage_analytics(range_days) {
            shell
                .usage_credits
                .apply_live_sources(None, Some(&analytics), range_days);
        }

        Ok(TelegramBidirectionalProjection::from_agents_and_channels(
            &raw_agents,
            &raw_channels,
        ))
    }

    /// Reads real local SQLite `AgentUsageStore` host analytics for the last `range_days` UTC days.
    pub fn read_host_usage_analytics(&self, range_days: u32) -> Option<HostAnalytics> {
        let dates = super::secondary_pages::UsageAndCreditsState::utc_date_window(range_days);
        let start_date = dates.first()?.clone();
        let end_date = dates.last()?.clone();
        self.runtime
            .database()
            .usage_store()
            .read_host_analytics(&HostAnalyticsInput {
                agent_id: None,
                start_date,
                end_date,
                time_zone: "UTC".to_string(),
            })
            .ok()
    }
}

/// Reads real local SQLite `AgentUsageStore` host analytics directly from `<data_dir>/openbot.db`
/// for the last `range_days` UTC days, returning `None` cleanly if `openbot.db` does not exist.
pub fn load_openbot_host_analytics(data_dir: &Path, range_days: u32) -> Option<HostAnalytics> {
    let db_path = data_dir.join("openbot.db");
    if !db_path.is_file() {
        return None;
    }
    let db = OpenBotDatabase::open(&db_path).ok()?;
    let dates = super::secondary_pages::UsageAndCreditsState::utc_date_window(range_days);
    let start_date = dates.first()?.clone();
    let end_date = dates.last()?.clone();
    db.usage_store()
        .read_host_analytics(&HostAnalyticsInput {
            agent_id: None,
            start_date,
            end_date,
            time_zone: "UTC".to_string(),
        })
        .ok()
}

