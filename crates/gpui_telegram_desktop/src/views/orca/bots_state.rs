//! Headless-testable state & real controller layer for the Orca Bots GPUI surface.
//!
//! Separates pure/local runtime state transitions from GPUI `Render` so M6 tests can
//! verify Bots Sidebar, Bot Chat Tree, 4-tab Bot Home, Priority-Folded `"Needs You"`
//! Inbox, all 10 Native Chat Bot Cards, 24-Template Gallery, Group Rooms, Packages,
//! E2EE Team Share, Cross-App Import, and Transactional Hand-Over deterministically.

use std::collections::{BTreeMap, HashSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use bot_browser::DesignModeChatHandoff;
use orca_bots::{
    AnswerQuestionRequest, BotChatTreeBuilder, BotChatTreeView, BotImportEngine, BotPatch,
    BotTemplateInstaller, ConnectorAccessRequest, CreateBotGroupRequest, CreateBotItemRequest,
    CreateBotRequest, CreateWebhookRequest, ExportPackageOptions, GroupMessageRecord,
    GroupMessageSender, HandOverTarget, InstalledTemplateResult, ListBotItemsFilter,
    PostGroupMessageRequest, TidyUpRuntimeState, TidyUpSchedule, UpdateAvatarStyleRequest,
    WebhookCliView, WebhookCopyUrlCard,
};
use orca_core::bot_ecosystem::{
    BotChatLiveState, BotChatNode, BotChatNodeKind, BotImportCandidate, BotImportSource,
    BotTemplate, BotTemplateCategory, BotTextRiskReport, HandOverOutcome, HandOverSuggestion,
    NativeChatBotCard, NativeChatBotCardKind, SealedTeamShareOffer, TeamShareOfferPayload,
    TeamSharePublicIdentity, UsageLimitContinue, scan_bot_text_risks,
};
use orca_core::bot_groups::{
    BotGroup, BotGroupReplyMode, BotGroupSend, BotGroupThread, MentionCandidate, RemoteAsk,
    parse_bot_mentions_detailed,
};
use orca_core::bot_items::{
    AskSelection, BotItem, BotItemActor, BotItemStatus, NEEDS_YOU_FOLD_CAP, NeedsYouFoldResult,
    StatusTransitionRequest,
};
use orca_core::bot_look_around::{
    BotUpNext, LookAroundEvaluation, LookAroundEvery, LookAroundPatch, LookAroundState,
};
use orca_core::bot_memory::{
    BotMemoryAuthor, BotMemoryFact, BotMemoryHistoryRow, parse_memory_facts,
};
use orca_core::bot_preamble::{QuietBotAnswerRow, StreamingQuietMarkerFilter};
use orca_core::bot_types::{
    BotAgentId, BotAvatarOutfit, BotMascotId, BotOfferAnswerUpdate, OrcaBotRecord,
};
use orca_core::triggers_webhooks::{WebhookAdapterId, WebhookFilters};

use super::runtime::OrcaDesktopRuntime;
use super::security_surface::SecuritySurfaceModalState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotHomeTab {
    #[default]
    Chat,
    Items,
    UpNext,
    Settings,
}

impl BotHomeTab {
    pub const ALL: [Self; 4] = [Self::Chat, Self::Items, Self::UpNext, Self::Settings];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Items => "items",
            Self::UpNext => "up_next",
            Self::Settings => "settings",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotsInputTarget {
    #[default]
    None,
    BotChatDraft,
    NewBotName,
    NewItemTitle,
    MemoryForgetQuery,
    GroupMessageDraft,
    NewGroupName,
    NewThreadTitle,
    TemplateCustomName,
    PackageImportBase64,
    E2eePeerIdentityJson,
    E2eeOfferJson,
    ImportPasteText,
    HandOverWorkspaceName,
    HandOverRepoSlug,
    HandOverPrompt,
    HandOverCustomName,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BotDialogState {
    #[default]
    None,
    TemplateGallery {
        selected_category: Option<BotTemplateCategory>,
        show_skills: bool,
        custom_name: String,
        last_installed: Option<InstalledTemplateResult>,
    },
    GroupRoom {
        group_id: String,
        selected_thread_id: Option<String>,
        draft: String,
        new_thread_title: String,
    },
    CreateGroup {
        name: String,
        selected_bot_ids: Vec<String>,
    },
    PackageManager {
        exported_zip_b64: Option<String>,
        exported_file_count: usize,
        scan_report: Option<BotTextRiskReport>,
        runnable_warnings: Vec<String>,
        import_zip_b64: String,
        last_imported_bot: Option<String>,
    },
    TeamShare {
        peer_identity_json: String,
        sealed_offer_json: Option<String>,
        incoming_offer_json: String,
        pending_peer_verify: Option<TeamSharePublicIdentity>,
        offer_state_label: Option<String>,
    },
    CrossAppImport {
        selected_source: Option<BotImportSource>,
        discovered: Vec<BotImportCandidate>,
        paste_text: String,
        last_imported_bot: Option<String>,
    },
    HandOver {
        workspace_id: String,
        workspace_name: String,
        repo_slug: String,
        recent_prompt: String,
        custom_name: String,
        target_existing_bot_id: Option<String>,
        preview: Option<HandOverSuggestion>,
        last_outcome: Option<HandOverOutcome>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrcaBotsAction {
    OpenIdeWorkspace,
    NeedsYouBadgeChanged(usize),
    ShowToast(String),
    OpenSecurityModal(SecuritySurfaceModalState),
    CopyToClipboard(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotChatMessage {
    pub id: String,
    pub sender_label: String,
    pub is_owner: bool,
    pub text: String,
    pub created_at_ms: i64,
    pub suppressed_quiet: bool,
}

#[derive(Debug, Clone)]
pub struct OrcaBotsSurfaceState {
    pub bots: Vec<OrcaBotRecord>,
    pub selected_bot_id: Option<String>,
    pub active_tab: BotHomeTab,
    pub selected_chat_session_id: Option<String>,
    pub chat_tree: Option<BotChatTreeView>,
    pub chat_messages: BTreeMap<String, Vec<BotChatMessage>>,
    pub chat_cards: Vec<NativeChatBotCard>,
    pub chat_draft: String,
    pub usage_limit: Option<UsageLimitContinue>,
    pub streaming_filter: StreamingQuietMarkerFilter,
    pub streaming_visible_preview: String,
    pub items: Vec<BotItem>,
    pub items_read_only: bool,
    pub needs_you_fold: NeedsYouFoldResult<BotItem>,
    pub needs_you_expanded: bool,
    pub focused_needs_you_uid: Option<String>,
    pub ask_draft_selections: BTreeMap<(String, usize), Vec<String>>,
    pub new_item_title: String,
    pub up_next: BotUpNext,
    pub look_around_state: LookAroundState,
    pub look_around_due: Option<LookAroundEvaluation>,
    pub soul_md: String,
    pub memory_md: String,
    pub memory_facts: Vec<BotMemoryFact>,
    pub memory_history: Vec<BotMemoryHistoryRow>,
    pub memory_forget_query: String,
    pub tidy_up_state: TidyUpRuntimeState,
    pub access_requests: Vec<ConnectorAccessRequest>,
    pub webhooks: Vec<WebhookCliView>,
    pub last_webhook_copy_card: Option<WebhookCopyUrlCard>,
    pub automations_count: usize,
    pub relay_running: bool,
    pub relay_base_url: Option<String>,
    pub groups: Vec<BotGroup>,
    pub selected_group_id: Option<String>,
    pub group_threads: Vec<BotGroupThread>,
    pub group_messages: Vec<GroupMessageRecord>,
    pub group_pending_count: usize,
    pub group_remote_asks: Vec<RemoteAsk>,
    pub dialog: BotDialogState,
    pub active_input: BotsInputTarget,
    pub new_bot_name: String,
    pub pending_security_modal: Option<SecuritySurfaceModalState>,
    pub pending_design_handoff: Option<DesignModeChatHandoff>,
    pub status_banner: Option<String>,
    pub error_banner: Option<String>,
}

impl Default for OrcaBotsSurfaceState {
    fn default() -> Self {
        Self {
            bots: Vec::new(),
            selected_bot_id: None,
            active_tab: BotHomeTab::Chat,
            selected_chat_session_id: None,
            chat_tree: None,
            chat_messages: BTreeMap::new(),
            chat_cards: Vec::new(),
            chat_draft: String::new(),
            usage_limit: None,
            streaming_filter: StreamingQuietMarkerFilter::new(),
            streaming_visible_preview: String::new(),
            items: Vec::new(),
            items_read_only: false,
            needs_you_fold: NeedsYouFoldResult {
                visible: Vec::new(),
                folded: Vec::new(),
                hidden_count: 0,
                approval_count: 0,
                focused_preserved: false,
            },
            needs_you_expanded: false,
            focused_needs_you_uid: None,
            ask_draft_selections: BTreeMap::new(),
            new_item_title: String::new(),
            up_next: BotUpNext::default(),
            look_around_state: LookAroundState::default(),
            look_around_due: None,
            soul_md: String::new(),
            memory_md: String::new(),
            memory_facts: Vec::new(),
            memory_history: Vec::new(),
            memory_forget_query: String::new(),
            tidy_up_state: TidyUpRuntimeState::default(),
            access_requests: Vec::new(),
            webhooks: Vec::new(),
            last_webhook_copy_card: None,
            automations_count: 0,
            relay_running: false,
            relay_base_url: None,
            groups: Vec::new(),
            selected_group_id: None,
            group_threads: Vec::new(),
            group_messages: Vec::new(),
            group_pending_count: 0,
            group_remote_asks: Vec::new(),
            dialog: BotDialogState::None,
            active_input: BotsInputTarget::None,
            new_bot_name: String::new(),
            pending_security_modal: None,
            pending_design_handoff: None,
            status_banner: None,
            error_banner: None,
        }
    }
}

impl OrcaBotsSurfaceState {
    pub const fn supported_card_kinds() -> &'static [NativeChatBotCardKind; 10] {
        BotChatTreeBuilder::all_card_kinds()
    }

    pub fn selected_bot(&self) -> Option<&OrcaBotRecord> {
        let id = self.selected_bot_id.as_deref()?;
        self.bots.iter().find(|b| b.id == id)
    }

    pub fn select_bot(&mut self, bot_id: &str) {
        self.selected_bot_id = Some(bot_id.to_string());
        self.selected_group_id = None;
        if let Some(bot) = self.bots.iter().find(|b| b.id == bot_id) {
            self.selected_chat_session_id = Some(
                bot.chat_session_id
                    .clone()
                    .unwrap_or_else(|| format!("home:{}", bot.id)),
            );
        }
    }

    pub fn select_home_tab(&mut self, tab: BotHomeTab) {
        self.active_tab = tab;
    }

    pub fn filter_templates(&self, category: Option<BotTemplateCategory>) -> Vec<&'static BotTemplate> {
        match category {
            Some(cat) => BotTemplateInstaller::list_templates_by_category(cat),
            None => BotTemplateInstaller::list_templates().iter().collect(),
        }
    }

    pub fn preview_hand_over(
        &mut self,
        workspace_name: &str,
        repo_slug: Option<&str>,
        recent_prompt: Option<&str>,
    ) -> Result<HandOverSuggestion, String> {
        let suggestion = orca_core::bot_ecosystem::suggest_hand_over_bot(
            workspace_name,
            repo_slug,
            recent_prompt,
        )
        .map_err(|e| e.to_string())?;
        if let BotDialogState::HandOver {
            ref mut preview,
            ref mut custom_name,
            ..
        } = self.dialog
        {
            if custom_name.trim().is_empty() {
                *custom_name = suggestion.suggested_bot_name.as_str().to_string();
            }
            *preview = Some(suggestion.clone());
        }
        Ok(suggestion)
    }

    /// Streams a bot reply chunk through `StreamingQuietMarkerFilter` so `NO_REPLY`
    /// stays suppressed while live tokens appear incrementally.
    pub fn push_stream_chunk_filtered(&mut self, chunk: &str) -> String {
        let visible = self.streaming_filter.push_chunk(chunk);
        self.streaming_visible_preview = visible.clone();
        visible
    }

    pub fn finish_stream_reply(
        &mut self,
        bot_id: &str,
        in_automation_turn: bool,
        now_ms: i64,
    ) -> Option<String> {
        let filter = std::mem::replace(&mut self.streaming_filter, StreamingQuietMarkerFilter::new());
        let outcome = filter.finish(QuietBotAnswerRow {
            bot_answer: true,
            streaming: false,
            in_automation_turn,
            in_acting_turn: false,
        });
        self.streaming_visible_preview.clear();
        let session_id = self
            .selected_chat_session_id
            .clone()
            .unwrap_or_else(|| format!("home:{bot_id}"));
        let text = outcome.display_text()?.to_string();
        let sender_label = self
            .bots
            .iter()
            .find(|b| b.id == bot_id)
            .map(|b| b.name.as_str().to_string())
            .unwrap_or_else(|| "Bot".to_string());
        let msg_seq = self.chat_messages.len();
        self.chat_messages
            .entry(session_id)
            .or_default()
            .push(BotChatMessage {
                id: format!("msg_{now_ms}_{msg_seq}"),
                sender_label,
                is_owner: false,
                text: text.clone(),
                created_at_ms: now_ms,
                suppressed_quiet: outcome.quiet,
            });
        Some(text)
    }

    /// Refreshes all visible bot fleet, items, Needs You inbox, Up Next, memory,
    /// access requests, webhooks, automations, groups, and chat tree state from
    /// the real local `OrcaBotsFacade`.
    pub fn refresh_from_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<(), String> {
        let facade = runtime.bots();
        self.bots = runtime.block_on_local(facade.controller().list_bots());
        if self.selected_bot_id.is_none()
            || !self
                .bots
                .iter()
                .any(|b| Some(b.id.as_str()) == self.selected_bot_id.as_deref())
        {
            self.selected_bot_id = self.bots.first().map(|b| b.id.clone());
        }
        self.items_read_only = facade.items_store().is_read_only().unwrap_or(false);
        self.groups = facade
            .group_runtime()
            .list_groups()
            .map_err(|e| e.to_string())?;
        let relay_opt = runtime.block_on_local(facade.tool_relay());
        self.relay_running = relay_opt.is_some();
        self.relay_base_url = relay_opt.map(|r| r.base_url());

        let selected_id = self.selected_bot_id.clone();
        self.compute_needs_you_fold(
            runtime,
            selected_id.as_deref(),
            self.focused_needs_you_uid.clone().as_deref(),
            now_ms,
        )?;

        if let Some(bot) = self.selected_bot().cloned() {
            self.refresh_selected_bot_details(runtime, &bot, now_ms)?;
        } else {
            self.items.clear();
            self.chat_tree = None;
            self.soul_md.clear();
            self.memory_md.clear();
            self.memory_facts.clear();
            self.memory_history.clear();
            self.access_requests.clear();
            self.webhooks.clear();
            self.up_next = BotUpNext::default();
            self.look_around_due = None;
        }
        if let Some(gid) = self.selected_group_id.clone() {
            let _ = self.refresh_group_room(runtime, &gid);
        }
        Ok(())
    }
}

impl OrcaBotsSurfaceState {
    fn refresh_selected_bot_details(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        bot: &OrcaBotRecord,
        now_ms: i64,
    ) -> Result<(), String> {
        let facade = runtime.bots();
        let home_dir = facade.home_store().bot_dir(&bot.folder);
        self.soul_md = facade
            .home_store()
            .read_text_optional(&home_dir.join("SOUL.md"))
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        self.memory_md = facade
            .home_store()
            .read_text_optional(&home_dir.join("MEMORY.md"))
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        self.memory_facts = parse_memory_facts(&self.memory_md);
        self.memory_history = facade
            .memory_service()
            .list_history(&bot.id)
            .map_err(|e| e.to_string())?;
        self.tidy_up_state = facade
            .memory_service()
            .get_tidy_up_state(&bot.id)
            .map_err(|e| e.to_string())?;

        self.items = facade
            .items_store()
            .list_items(ListBotItemsFilter {
                owner_bot_id: Some(bot.id.clone()),
                statuses: None,
                include_closed: true,
                include_snoozed: true,
                now_ms: Some(now_ms),
                limit: Some(100),
                offset: 0,
            })
            .map_err(|e| e.to_string())?;

        let up_next_state = facade
            .up_next_store()
            .get_state(&bot.id)
            .map_err(|e| e.to_string())?;
        self.look_around_state = up_next_state.schedule.clone();
        self.up_next = facade
            .up_next_store()
            .list_suggestions(&bot.id)
            .map_err(|e| e.to_string())?;
        self.look_around_due = facade
            .look_around()
            .evaluate_due(&bot.id, bot.agent, now_ms, 0)
            .ok();

        self.access_requests = facade
            .access_host()
            .list_requests(Some(&bot.id), None)
            .map_err(|e| e.to_string())?;
        self.webhooks = facade
            .webhook_engine()
            .list_cli_views(Some(&bot.id))
            .map_err(|e| e.to_string())?;
        self.automations_count = facade
            .automation_engine()
            .list_automations(Some(&bot.id))
            .map(|a| a.len())
            .unwrap_or(0);

        let home_session = bot
            .chat_session_id
            .clone()
            .unwrap_or_else(|| format!("home:{}", bot.id));
        if self.selected_chat_session_id.is_none() {
            self.selected_chat_session_id = Some(home_session.clone());
        }
        let has_needs_you = self
            .items
            .iter()
            .any(|i| i.status == BotItemStatus::NeedsYou);
        let mut nodes = vec![BotChatNode {
            session_id: home_session.clone(),
            parent_session_id: None,
            title: format!("{} Home", bot.name.as_str()),
            kind: BotChatNodeKind::Home,
            live_state: if has_needs_you {
                BotChatLiveState::NeedsYou
            } else {
                BotChatLiveState::Idle
            },
            note: None,
            unread: false,
            needs_you: has_needs_you,
            updated_at_ms: bot.updated_at.max(now_ms),
        }];
        if bot.connectors.slack.is_some() {
            nodes.push(BotChatNode {
                session_id: format!("slack:{}", bot.id),
                parent_session_id: Some(home_session.clone()),
                title: "Slack threads".to_string(),
                kind: BotChatNodeKind::Slack,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: bot.updated_at,
            });
        }
        if bot.connectors.github.is_some() {
            nodes.push(BotChatNode {
                session_id: format!("github:{}", bot.id),
                parent_session_id: Some(home_session.clone()),
                title: "GitHub watches".to_string(),
                kind: BotChatNodeKind::Github,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: bot.updated_at,
            });
        }
        if bot.connectors.linear.is_some() {
            nodes.push(BotChatNode {
                session_id: format!("linear:{}", bot.id),
                parent_session_id: Some(home_session.clone()),
                title: "Linear issues".to_string(),
                kind: BotChatNodeKind::Linear,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: bot.updated_at,
            });
        }
        let cards_for_bot: Vec<NativeChatBotCard> = self
            .chat_cards
            .iter()
            .filter(|c| c.bot_id == bot.id)
            .cloned()
            .collect();
        self.chat_tree = facade
            .build_chat_tree(
                &bot.id,
                &nodes,
                self.usage_limit.clone(),
                cards_for_bot,
                now_ms,
            )
            .ok();
        Ok(())
    }

    pub fn compute_needs_you_fold(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        bot_id: Option<&str>,
        focused_uid: Option<&str>,
        now_ms: i64,
    ) -> Result<&NeedsYouFoldResult<BotItem>, String> {
        let fold = runtime
            .bots()
            .items_store()
            .needs_you_inbox(bot_id, focused_uid, Some(NEEDS_YOU_FOLD_CAP), now_ms)
            .map_err(|e| e.to_string())?;
        self.focused_needs_you_uid = focused_uid.map(str::to_string);
        self.needs_you_fold = fold;
        Ok(&self.needs_you_fold)
    }

    pub fn toggle_ask_option(
        &mut self,
        item_uid: &str,
        question_idx: usize,
        label: &str,
        multi: bool,
    ) {
        let entry = self
            .ask_draft_selections
            .entry((item_uid.to_string(), question_idx))
            .or_default();
        if multi {
            if let Some(pos) = entry.iter().position(|x| x == label) {
                entry.remove(pos);
            } else {
                entry.push(label.to_string());
            }
        } else {
            entry.clear();
            entry.push(label.to_string());
        }
    }

    pub fn answer_ask_item(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        item_uid: &str,
        question_event: u64,
        selections: Vec<AskSelection>,
        now_ms: i64,
    ) -> Result<BotItem, String> {
        let (updated, _) = runtime
            .bots()
            .items_store()
            .answer_question(
                item_uid,
                AnswerQuestionRequest {
                    actor: BotItemActor::Owner,
                    question_event,
                    selections: Some(selections),
                    reply_text: None,
                    files: Vec::new(),
                },
                now_ms,
            )
            .map_err(|e| e.to_string())?;
        self.ask_draft_selections
            .retain(|(uid, _), _| uid != item_uid);
        self.status_banner = Some(format!("Answered item #{}", updated.number));
        self.error_banner = None;
        let _ = self.refresh_from_local(runtime, now_ms);
        Ok(updated)
    }
}

impl OrcaBotsSurfaceState {
    pub fn create_bot_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        name: Option<String>,
        agent: BotAgentId,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let req = CreateBotRequest {
            name: name.filter(|s| !s.trim().is_empty()),
            agent,
            first_run: true,
            ..Default::default()
        };
        let bot = runtime
            .block_on_local(runtime.bots().create_bot(req))
            .map_err(|e| e.to_string())?;
        self.selected_bot_id = Some(bot.id.clone());
        self.new_bot_name.clear();
        self.status_banner = Some(format!("Created bot '{}'", bot.name.as_str()));
        self.error_banner = None;
        self.refresh_from_local(runtime, now_ms)?;
        Ok(bot)
    }

    pub fn delete_selected_bot_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        runtime
            .block_on_local(runtime.bots().delete_bot(&bot_id))
            .map_err(|e| e.to_string())?;
        self.selected_bot_id = None;
        self.status_banner = Some("Bot deleted".to_string());
        self.error_banner = None;
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn toggle_bot_paused_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        let updated = runtime
            .block_on_local(runtime.bots().controller().update_bot(
                &bot.id,
                BotPatch {
                    paused: Some(!bot.paused),
                    ..Default::default()
                },
            ))
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(if updated.paused {
            format!("Paused '{}'", updated.name.as_str())
        } else {
            format!("Resumed '{}'", updated.name.as_str())
        });
        self.refresh_from_local(runtime, now_ms)?;
        Ok(updated)
    }

    /// Host-only MCP servers toggle — exclusively uses `BotController::set_host_mcp_servers`.
    pub fn set_host_mcp_servers_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        enabled: bool,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        let updated = runtime
            .block_on_local(
                runtime
                    .bots()
                    .controller()
                    .set_host_mcp_servers(&bot_id, enabled),
            )
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!(
            "Host MCP servers {}",
            if enabled { "enabled" } else { "disabled" }
        ));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(updated)
    }

    /// CAS-guarded mascot / color / outfit avatar update.
    pub fn update_avatar_style_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        mascot: Option<BotMascotId>,
        color: Option<String>,
        outfit: Option<BotAvatarOutfit>,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        let req = UpdateAvatarStyleRequest {
            avatar: mascot.map(|m| format!("mascot:{}", m.as_str())),
            color,
            outfit: outfit.map(|o| o.as_str().to_string()),
            expected_avatar_revision: Some(bot.avatar_spec.avatar_revision),
            expected_avatar_revision_epoch: Some(bot.avatar_spec.avatar_revision_epoch.clone()),
        };
        let updated = runtime
            .block_on_local(
                runtime
                    .bots()
                    .controller()
                    .update_avatar_style(&bot.id, req),
            )
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!(
            "Avatar updated (rev {})",
            updated.avatar_spec.avatar_revision
        ));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(updated)
    }

    pub fn apply_offer_card_action(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        offer_id: &str,
        sent: bool,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        let state = if sent {
            orca_core::bot_types::BotOfferAnswerState::Sent
        } else {
            orca_core::bot_types::BotOfferAnswerState::Dismissed
        };
        let updated = runtime
            .block_on_local(runtime.bots().controller().update_bot(
                &bot_id,
                BotPatch {
                    offer_answer: Some(BotOfferAnswerUpdate::Claim {
                        offer_id: offer_id.to_string(),
                        state,
                        answered_at: now_ms,
                    }),
                    ..Default::default()
                },
            ))
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!("Offer '{offer_id}' answered"));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(updated)
    }

    pub fn send_chat_message_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        let draft = self.chat_draft.trim().to_string();
        if draft.is_empty() {
            return Ok(());
        }
        let session_id = self
            .selected_chat_session_id
            .clone()
            .unwrap_or_else(|| format!("home:{}", bot.id));
        // Build real authority preamble for this turn to verify home integrity
        let _ = runtime
            .bots()
            .memory_service()
            .build_session_preamble(&bot, Some("fabushi"), None);
        let msg_seq = self.chat_messages.len();
        self.chat_messages
            .entry(session_id)
            .or_default()
            .push(BotChatMessage {
                id: format!("owner_{now_ms}_{msg_seq}"),
                sender_label: "You".to_string(),
                is_owner: true,
                text: draft,
                created_at_ms: now_ms,
                suppressed_quiet: false,
            });
        self.chat_draft.clear();
        self.pending_design_handoff = None;
        self.status_banner = Some(format!("Message queued for '{}'", bot.name.as_str()));
        self.error_banner = None;
        Ok(())
    }

    pub fn inject_design_mode_handoff(&mut self, handoff: DesignModeChatHandoff) {
        if self.chat_draft.trim().is_empty() {
            self.chat_draft = handoff.prompt_markdown.clone();
        } else {
            self.chat_draft.push_str("\n\n");
            self.chat_draft.push_str(&handoff.prompt_markdown);
        }
        self.active_tab = BotHomeTab::Chat;
        self.status_banner = Some(format!(
            "Attached Design Mode capture ({}) to bot chat draft",
            handoff.selector
        ));
        self.pending_design_handoff = Some(handoff);
    }
}

impl OrcaBotsSurfaceState {
    pub fn create_item_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        title: String,
        status: BotItemStatus,
        now_ms: i64,
    ) -> Result<BotItem, String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        let mut req = CreateBotItemRequest::new(bot_id, BotItemActor::Owner, title);
        req.status = status;
        let (item, _) = runtime
            .bots()
            .items_store()
            .create_item(req, now_ms)
            .map_err(|e| e.to_string())?;
        self.new_item_title.clear();
        self.status_banner = Some(format!("Created item #{}: {}", item.number, item.title));
        self.error_banner = None;
        self.refresh_from_local(runtime, now_ms)?;
        Ok(item)
    }

    pub fn transition_item_status_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        item_uid: &str,
        to_status: BotItemStatus,
        expected_version: u64,
        now_ms: i64,
    ) -> Result<BotItem, String> {
        let updated = if to_status.is_closed() {
            let (item, _) = runtime
                .block_on_local(runtime.bots().close_item(
                    item_uid,
                    BotItemActor::Owner,
                    to_status,
                    None,
                    Some(expected_version),
                    None,
                    now_ms,
                ))
                .map_err(|e| e.to_string())?;
            item
        } else {
            let (item, _) = runtime
                .bots()
                .items_store()
                .set_status(
                    item_uid,
                    StatusTransitionRequest {
                        actor: BotItemActor::Owner,
                        to_status,
                        expected_version: Some(expected_version),
                        wait_note: None,
                        close_note: None,
                        note: None,
                        ask: None,
                    },
                    now_ms,
                )
                .map_err(|e| e.to_string())?;
            item
        };
        self.status_banner = Some(format!(
            "Item #{} -> {}",
            updated.number,
            updated.status.as_str()
        ));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(updated)
    }

    pub fn snooze_item_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        item_uid: &str,
        until_ms: i64,
        now_ms: i64,
    ) -> Result<BotItem, String> {
        let (item, _) = runtime
            .bots()
            .items_store()
            .snooze_item(item_uid, BotItemActor::Owner, until_ms, now_ms)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!("Snoozed item #{}", item.number));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(item)
    }

    pub fn unsnooze_item_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        item_uid: &str,
        now_ms: i64,
    ) -> Result<BotItem, String> {
        let (item, _) = runtime
            .bots()
            .items_store()
            .unsnooze_item(item_uid, BotItemActor::Owner, now_ms)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!("Unsnoozed item #{}", item.number));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(item)
    }

    pub fn patch_look_around_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        on: Option<bool>,
        every: Option<LookAroundEvery>,
        weekdays_only: Option<bool>,
        now_ms: i64,
    ) -> Result<LookAroundState, String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        let state = runtime
            .bots()
            .up_next_store()
            .patch_schedule(
                &bot.id,
                bot.agent,
                LookAroundPatch {
                    on,
                    every,
                    weekdays_only,
                    focus: None,
                    since_ms: None,
                },
                now_ms,
            )
            .map_err(|e| e.to_string())?;
        self.look_around_state = state.clone();
        self.status_banner = Some(format!(
            "Look Around {} ({})",
            if state.on { "ON" } else { "OFF" },
            state.schedule.every.as_str()
        ));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(state)
    }

    pub fn start_up_next_suggestion_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        suggestion_id: &str,
        now_ms: i64,
    ) -> Result<String, String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        let started = runtime
            .bots()
            .up_next_store()
            .start_suggestion(&bot_id, suggestion_id)
            .map_err(|e| e.to_string())?;
        self.chat_draft = started.composer_ask.clone();
        self.active_tab = BotHomeTab::Chat;
        self.status_banner = Some(format!("Started '{}'", started.suggestion.title));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(started.composer_ask)
    }

    pub fn dismiss_up_next_suggestion_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        suggestion_id: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        runtime
            .bots()
            .up_next_store()
            .dismiss_suggestion(&bot_id, suggestion_id, now_ms)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some("Suggestion dismissed".to_string());
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn forget_memory_fact_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        query: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        let res = runtime
            .bots()
            .memory_service()
            .forget_fact(&bot, query, BotMemoryAuthor::Owner, now_ms)
            .map_err(|e| e.to_string())?;
        self.memory_forget_query.clear();
        self.status_banner = Some(format!(
            "Forgot {} fact(s)",
            res.history_entry.change.removed.len()
        ));
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn undo_memory_change_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        history_row_id: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        runtime
            .bots()
            .memory_service()
            .undo_memory_change(&bot, history_row_id, now_ms)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some("Memory change undone".to_string());
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn configure_tidy_up_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        enabled: bool,
        schedule: TidyUpSchedule,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        runtime
            .bots()
            .memory_service()
            .configure_tidy_up(&bot_id, enabled, schedule, now_ms)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!(
            "Overnight Tidy-Up (3–8 AM) {}",
            if enabled { "enabled" } else { "disabled" }
        ));
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn undo_tidy_up_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        row_id: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "No bot selected".to_string())?;
        runtime
            .bots()
            .memory_service()
            .undo_tidy_up(&bot, Some(row_id), now_ms)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some("Tidy-Up change undone".to_string());
        self.refresh_from_local(runtime, now_ms)
    }
}

impl OrcaBotsSurfaceState {
    pub fn request_access_approval_modal(
        &mut self,
        request_id: &str,
    ) -> Result<SecuritySurfaceModalState, String> {
        let req = self
            .access_requests
            .iter()
            .find(|r| r.request_id == request_id)
            .ok_or_else(|| format!("Access request '{request_id}' not found"))?;
        let modal = SecuritySurfaceModalState::open_bot_access_approval(req);
        self.pending_security_modal = Some(modal.clone());
        Ok(modal)
    }

    pub fn decide_access_request_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        request_id: &str,
        approve: bool,
        now_ms: i64,
    ) -> Result<(), String> {
        runtime
            .block_on_local(runtime.bots().access_host().decide_request(
                request_id,
                approve,
                now_ms,
                runtime.bots().controller(),
            ))
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!(
            "Access request {} {}",
            request_id,
            if approve { "approved" } else { "declined" }
        ));
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn create_webhook_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        adapter: WebhookAdapterId,
        now_ms: i64,
    ) -> Result<WebhookCopyUrlCard, String> {
        let bot_id = self
            .selected_bot_id
            .clone()
            .ok_or_else(|| "No bot selected".to_string())?;
        let (view, card) = runtime
            .bots()
            .webhook_engine()
            .create_hook(CreateWebhookRequest {
                bot_id: bot_id.clone(),
                automation_id: format!("auto_{now_ms}"),
                adapter,
                filters: WebhookFilters::default(),
                hourly_cap: None,
                grace_minutes: None,
                require_signature: true,
                now_ms,
            })
            .map_err(|e| e.to_string())?;
        let session_id = self
            .selected_chat_session_id
            .clone()
            .unwrap_or_else(|| format!("home:{bot_id}"));
        if let Ok(native_card) = BotChatTreeBuilder::make_card(
            NativeChatBotCardKind::Webhook,
            bot_id,
            session_id,
            format!("Webhook {} ({})", view.hook_id, adapter.as_str()),
            Some("One-time ingest URL & signing secret (UI only — never printed in CLI)".to_string()),
            serde_json::json!({
                "hookId": card.hook_id,
                "ingestUrl": card.ingest_url,
                "secretKey": card.secret_key,
                "adapter": card.adapter.as_str(),
                "epoch": card.epoch,
            }),
            now_ms,
        ) {
            self.chat_cards.push(native_card);
        }
        self.last_webhook_copy_card = Some(card.clone());
        self.status_banner = Some(format!("Webhook {} created", view.hook_id));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(card)
    }

    pub fn rotate_webhook_secret_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        hook_id: &str,
        now_ms: i64,
    ) -> Result<WebhookCopyUrlCard, String> {
        let card = runtime
            .bots()
            .webhook_engine()
            .rotate_hook_secret(hook_id)
            .map_err(|e| e.to_string())?;
        self.last_webhook_copy_card = Some(card.clone());
        self.status_banner = Some(format!("Rotated secret for {hook_id} (epoch {})", card.epoch));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(card)
    }

    pub fn create_group_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        name: String,
        member_bot_ids: Vec<String>,
        now_ms: i64,
    ) -> Result<BotGroup, String> {
        let members = if member_bot_ids.is_empty() {
            self.bots.iter().take(2).map(|b| b.id.clone()).collect()
        } else {
            member_bot_ids
        };
        let group = runtime
            .bots()
            .group_runtime()
            .create_group(CreateBotGroupRequest {
                name,
                folder: None,
                member_bot_ids: members,
                reply_mode: BotGroupReplyMode::Mentions,
                remote_members: Vec::new(),
            })
            .map_err(|e| e.to_string())?;
        self.selected_group_id = Some(group.id.clone());
        self.dialog = BotDialogState::GroupRoom {
            group_id: group.id.clone(),
            selected_thread_id: None,
            draft: String::new(),
            new_thread_title: String::new(),
        };
        self.status_banner = Some(format!("Created group '{}'", group.name));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(group)
    }

    pub fn refresh_group_room(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        group_id: &str,
    ) -> Result<(), String> {
        let rt = runtime.bots().group_runtime();
        self.group_threads = rt.list_threads(group_id).map_err(|e| e.to_string())?;
        let thread_id = match &self.dialog {
            BotDialogState::GroupRoom {
                selected_thread_id, ..
            } => selected_thread_id.as_deref(),
            _ => None,
        };
        self.group_messages = rt
            .list_messages(group_id, thread_id)
            .map_err(|e| e.to_string())?;
        self.group_remote_asks = rt.list_remote_asks(group_id).map_err(|e| e.to_string())?;
        let mut pending_total = 0usize;
        if let Some(group) = self.groups.iter().find(|g| g.id == group_id) {
            for bid in &group.member_bot_ids {
                pending_total += rt.list_pending_for_bot(bid).map(|v| v.len()).unwrap_or(0);
            }
        }
        self.group_pending_count = pending_total;
        Ok(())
    }

    pub fn set_group_reply_mode_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        group_id: &str,
        mode: BotGroupReplyMode,
        now_ms: i64,
    ) -> Result<(), String> {
        runtime
            .bots()
            .group_runtime()
            .update_reply_mode(group_id, mode)
            .map_err(|e| e.to_string())?;
        self.refresh_from_local(runtime, now_ms)?;
        self.refresh_group_room(runtime, group_id)
    }

    pub fn create_group_thread_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        group_id: &str,
        first_post: &str,
    ) -> Result<BotGroupThread, String> {
        let thread = runtime
            .bots()
            .group_runtime()
            .create_thread(group_id, first_post)
            .map_err(|e| e.to_string())?;
        if let BotDialogState::GroupRoom {
            ref mut selected_thread_id,
            ref mut new_thread_title,
            ..
        } = self.dialog
        {
            *selected_thread_id = Some(thread.id.clone());
            new_thread_title.clear();
        }
        self.refresh_group_room(runtime, group_id)?;
        Ok(thread)
    }

    pub fn post_group_message_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        group_id: &str,
        thread_id: Option<String>,
        text: String,
    ) -> Result<usize, String> {
        let group = self
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .cloned()
            .ok_or_else(|| format!("Group '{group_id}' not found"))?;
        let candidates: Vec<MentionCandidate> = self
            .bots
            .iter()
            .filter(|b| group.member_bot_ids.iter().any(|id| id == &b.id))
            .map(|b| MentionCandidate {
                id: b.id.clone(),
                name: b.name.as_str().to_string(),
                is_remote: false,
            })
            .collect();
        let paused_bot_ids: HashSet<String> = self
            .bots
            .iter()
            .filter(|b| b.paused)
            .map(|b| b.id.clone())
            .collect();
        let parsed = parse_bot_mentions_detailed(&text, &candidates);
        let posted = runtime
            .bots()
            .group_runtime()
            .post_message(PostGroupMessageRequest {
                group_id: group_id.to_string(),
                thread_id,
                sender: GroupMessageSender::Owner,
                send: BotGroupSend {
                    text,
                    images: Vec::new(),
                },
                active_local_candidates: candidates,
                paused_bot_ids,
                requester: None,
                peer_scope: None,
                synthetic_root: None,
            })
            .map_err(|e| e.to_string())?;
        if let BotDialogState::GroupRoom { ref mut draft, .. } = self.dialog {
            draft.clear();
        }
        self.status_banner = Some(format!(
            "Group message posted (mentions={}, held={})",
            parsed.members.len(),
            posted.pending_ids.len()
        ));
        self.refresh_group_room(runtime, group_id)?;
        Ok(posted.pending_ids.len())
    }
}

impl OrcaBotsSurfaceState {
    pub fn flush_paused_group_mail_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        group_id: &str,
    ) -> Result<usize, String> {
        let group = self
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .cloned()
            .ok_or_else(|| format!("Group '{group_id}' not found"))?;
        let mut flushed = 0usize;
        for bid in &group.member_bot_ids {
            if let Ok(items) = runtime.bots().group_runtime().flush_pending_for_bot(bid) {
                flushed += items.len();
            }
        }
        self.status_banner = Some(format!("Flushed {flushed} held group message(s)"));
        self.refresh_group_room(runtime, group_id)?;
        Ok(flushed)
    }

    pub fn install_template_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        template_id: &str,
        custom_name: Option<&str>,
        now_ms: i64,
    ) -> Result<InstalledTemplateResult, String> {
        let installed = runtime
            .block_on_local(
                runtime
                    .bots()
                    .install_from_template(template_id, custom_name),
            )
            .map_err(|e| e.to_string())?;
        self.selected_bot_id = Some(installed.bot.id.clone());
        if let BotDialogState::TemplateGallery {
            ref mut last_installed,
            ref mut custom_name,
            ..
        } = self.dialog
        {
            *last_installed = Some(installed.clone());
            custom_name.clear();
        }
        self.status_banner = Some(format!(
            "Installed '{}' (schedules disabled by default)",
            installed.bot.name.as_str()
        ));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(installed)
    }

    pub fn install_bundled_skill_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        skill_slug: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "Select a bot before installing a skill".to_string())?;
        BotTemplateInstaller::install_bundled_skill(runtime.bots().home_store(), &bot, skill_slug)
            .map_err(|e| e.to_string())?;
        self.status_banner = Some(format!(
            "Installed skill '{skill_slug}' into '{}'",
            bot.name.as_str()
        ));
        self.refresh_from_local(runtime, now_ms)
    }

    pub fn export_selected_bot_package_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
    ) -> Result<(), String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "Select a bot to export".to_string())?;
        let combined_text = format!("{}\n{}", self.soul_md, self.memory_md);
        let risk_report = scan_bot_text_risks(&combined_text);
        match runtime.block_on_local(
            runtime
                .bots()
                .export_package(&bot.id, &ExportPackageOptions::default()),
        ) {
            Ok(exported) => {
                let b64 = BASE64_STANDARD.encode(&exported.zip_bytes);
                let count = exported.validated.manifest.parts.len();
                let runnable = exported.runnable_warnings.clone();
                self.dialog = BotDialogState::PackageManager {
                    exported_zip_b64: Some(b64),
                    exported_file_count: count,
                    scan_report: Some(risk_report),
                    runnable_warnings: runnable,
                    import_zip_b64: String::new(),
                    last_imported_bot: None,
                };
                self.status_banner = Some(format!("Exported package ({count} files)"));
                self.error_banner = None;
                Ok(())
            }
            Err(err) => {
                self.dialog = BotDialogState::PackageManager {
                    exported_zip_b64: None,
                    exported_file_count: 0,
                    scan_report: Some(risk_report),
                    runnable_warnings: Vec::new(),
                    import_zip_b64: String::new(),
                    last_imported_bot: None,
                };
                let msg = err.to_string();
                self.error_banner = Some(msg.clone());
                Err(msg)
            }
        }
    }

    pub fn import_bot_package_b64_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        zip_b64: &str,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let bytes = BASE64_STANDARD
            .decode(zip_b64.trim())
            .map_err(|e| format!("Invalid base64 package: {e}"))?;
        let imported = runtime
            .block_on_local(runtime.bots().import_package(&bytes, None))
            .map_err(|e| e.to_string())?;
        self.selected_bot_id = Some(imported.bot.id.clone());
        if let BotDialogState::PackageManager {
            ref mut last_imported_bot,
            ref mut import_zip_b64,
            ..
        } = self.dialog
        {
            *last_imported_bot = Some(imported.bot.name.as_str().to_string());
            import_zip_b64.clear();
        }
        self.status_banner = Some(format!(
            "Imported bot package '{}'",
            imported.bot.name.as_str()
        ));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(imported.bot)
    }

    pub fn seal_team_share_offer_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        peer_identity_json: &str,
        now_ms: i64,
    ) -> Result<SealedTeamShareOffer, String> {
        let bot = self
            .selected_bot()
            .cloned()
            .ok_or_else(|| "Select a bot to share".to_string())?;
        let peer: TeamSharePublicIdentity = serde_json::from_str(peer_identity_json.trim())
            .map_err(|e| format!("Invalid peer identity JSON: {e}"))?;
        let my_id = runtime.bots().team_share().public_identity();
        let payload = TeamShareOfferPayload {
            share_id: format!("shr_{now_ms}"),
            bot_id: bot.id.clone(),
            bot_name: bot.name.as_str().to_string(),
            relay_url: runtime
                .bots()
                .config()
                .webhook_base_url
                .clone(),
            pinned_host_ed25519_b64: my_id.ed25519_public_b64,
            pinned_chat_session_id: bot.chat_session_id.clone(),
            sender_name: Some(my_id.host_id),
        };
        let offer = runtime
            .bots()
            .team_share()
            .create_offer(
                &peer,
                &payload,
                format!("off_{now_ms}"),
                now_ms + 24 * 60 * 60 * 1000,
            )
            .map_err(|e| e.to_string())?;
        let offer_json = serde_json::to_string_pretty(&offer).map_err(|e| e.to_string())?;
        if let BotDialogState::TeamShare {
            ref mut sealed_offer_json,
            ref mut offer_state_label,
            ..
        } = self.dialog
        {
            *sealed_offer_json = Some(offer_json);
            *offer_state_label = Some(format!("Sealed pending offer {}", offer.offer_id));
        }
        self.status_banner = Some(format!("Sealed E2EE offer for {}", peer.host_id));
        Ok(offer)
    }

    /// Routes peer identity verification for an incoming E2EE team share offer through
    /// the non-themable `SecuritySurfaceModal` before accepting/opening.
    pub fn request_e2ee_peer_verification(
        &mut self,
        peer_identity_json: &str,
    ) -> Result<SecuritySurfaceModalState, String> {
        let peer: TeamSharePublicIdentity = serde_json::from_str(peer_identity_json.trim())
            .map_err(|e| format!("Invalid peer identity JSON: {e}"))?;
        let modal = SecuritySurfaceModalState::open_e2ee_verification(
            &format!("security:team-share:{}", peer.host_id),
            &peer,
        );
        if let BotDialogState::TeamShare {
            ref mut pending_peer_verify,
            ..
        } = self.dialog
        {
            *pending_peer_verify = Some(peer);
        }
        self.pending_security_modal = Some(modal.clone());
        Ok(modal)
    }

    pub fn open_verified_team_share_offer_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        peer: TeamSharePublicIdentity,
        offer_json: &str,
        now_ms: i64,
    ) -> Result<TeamShareOfferPayload, String> {
        let sealed: SealedTeamShareOffer = serde_json::from_str(offer_json.trim())
            .map_err(|e| format!("Invalid sealed offer JSON: {e}"))?;
        let expected_ed = peer.ed25519_public_b64.clone();
        let opened = runtime
            .bots()
            .team_share()
            .accept_incoming_offer(Some(&expected_ed), peer, &sealed, now_ms)
            .map_err(|e| e.to_string())?;
        if let BotDialogState::TeamShare {
            ref mut offer_state_label,
            ref mut pending_peer_verify,
            ..
        } = self.dialog
        {
            *pending_peer_verify = None;
            *offer_state_label = Some(format!(
                "Verified & accepted '{}' (share_id={})",
                opened.bot_name, opened.share_id
            ));
        }
        self.status_banner = Some(format!("Accepted E2EE shared bot '{}'", opened.bot_name));
        Ok(opened)
    }

    pub fn discover_cross_app_imports_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        now_ms: i64,
    ) -> Result<usize, String> {
        let cands = runtime
            .bots()
            .discover_imports(now_ms)
            .map_err(|e| e.to_string())?;
        let count = cands.len();
        if let BotDialogState::CrossAppImport {
            ref mut discovered, ..
        } = self.dialog
        {
            *discovered = cands;
        }
        self.status_banner = Some(format!("Discovered {count} external bot candidate(s)"));
        Ok(count)
    }

    pub fn parse_generic_import_paste_local(
        &mut self,
        paste_text: &str,
        now_ms: i64,
    ) -> Result<usize, String> {
        let cands = BotImportEngine::parse_generic_export(paste_text, now_ms)
            .map_err(|e| e.to_string())?;
        let count = cands.len();
        if let BotDialogState::CrossAppImport {
            ref mut discovered, ..
        } = self.dialog
        {
            *discovered = cands;
        }
        self.status_banner = Some(format!("Parsed {count} candidate(s) from pasted export"));
        Ok(count)
    }

    pub fn import_discovered_candidate_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        candidate: &BotImportCandidate,
        raw_reply: Option<&str>,
        now_ms: i64,
    ) -> Result<OrcaBotRecord, String> {
        let bot = runtime
            .block_on_local(runtime.bots().import_candidate(candidate, None, raw_reply))
            .map_err(|e| e.to_string())?;
        self.selected_bot_id = Some(bot.id.clone());
        if let BotDialogState::CrossAppImport {
            ref mut last_imported_bot,
            ..
        } = self.dialog
        {
            *last_imported_bot = Some(bot.name.as_str().to_string());
        }
        self.status_banner = Some(format!("Imported '{}' with secret redaction", bot.name.as_str()));
        self.refresh_from_local(runtime, now_ms)?;
        Ok(bot)
    }

    pub fn execute_hand_over_local(
        &mut self,
        runtime: &OrcaDesktopRuntime,
        workspace_id: &str,
        target: HandOverTarget,
        now_ms: i64,
    ) -> Result<HandOverOutcome, String> {
        let outcome = runtime
            .block_on_local(
                runtime
                    .bots()
                    .hand_over_workspace(runtime.lineage().as_ref(), workspace_id, target),
            )
            .map_err(|e| e.to_string())?;
        if let BotDialogState::HandOver {
            ref mut last_outcome,
            ..
        } = self.dialog
        {
            *last_outcome = Some(outcome.clone());
        }
        match &outcome {
            HandOverOutcome::Created { bot_id, name, .. } => {
                self.selected_bot_id = Some(bot_id.clone());
                self.status_banner = Some(format!("Handed workspace to new bot '{}'", name.as_str()));
                self.error_banner = None;
            }
            HandOverOutcome::Attached { bot_id } => {
                self.selected_bot_id = Some(bot_id.clone());
                self.status_banner = Some(format!("Handed workspace to bot '{bot_id}'"));
                self.error_banner = None;
            }
            HandOverOutcome::RolledBack { placement, reason } => {
                self.error_banner = Some(format!(
                    "Hand-over rolled back ({placement:?}): {reason}"
                ));
            }
            HandOverOutcome::Declined | HandOverOutcome::Deferred => {
                self.status_banner = Some("Hand-over deferred".to_string());
            }
        }
        self.refresh_from_local(runtime, now_ms)?;
        Ok(outcome)
    }
}
