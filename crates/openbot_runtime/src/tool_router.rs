//! Unified OpenBot Tool Dispatch Engine (`OB-F10`).
//!
//! Connects `ToolCatalog` (160 built-in tools + JSON escape repair + recursive meta-tool
//! refusal + `[null]` structured item guard) to every core M3 runtime service:
//! - 3-Tier Memory (`<= 4,096 B` essential tier, FTS5 `bm25` + CJK regex, `site` recall,
//!   and strict `channel.isolated` separation from `projection_channel_memories`)
//! - Agent Conversation History (`history_search` / `history_read` with channel-context exclusion)
//! - Supervised `SharedAgentDataDb` SQL tools (`owner_agent_id` SQLite authorizer + `<= 64` tables)
//! - Agent Lifecycle, Providers, `DrainScheduler` (`Mailbox`, DMs, tasks, reactions, pins),
//!   `AttentionRegistry` (`ask_user` / `ask_ui` / approvals), `ChannelService` (all 15+ channel tools),
//!   `SkillService` (`SKILL.md` + `<= 32` cap), `McpGateway` (`<= 100` + PKCE + `computer_use`),
//!   `VaultService` (`ChaCha20Poly1305` + `0600` keys + TOTP + Link payment), and `BrowserService`
//!   (`openbot_browser.*` CDP tools + `fill_card` exfiltration lock + takeover).

use std::sync::Arc;

use openbot_contracts::agents::{
    AgentAccess, AgentBackupModels, AgentReasoningEffort, AgentTaskOrigin, UpdateAgentInput,
};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::conversation::{
    AgentPromptQuestion, ConversationReactionActor, ROUTINE_QUIET_ANSWER_ITEM_TYPE,
};
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_contracts::providers::AgentProviderId;
use openbot_contracts::ui_blocks::UiBlockSpec;
use openbot_storage::{
    AGENT_MEMORY_CONTEXT_BUDGET_BYTES, AgentMemory, AgentMemoryOrigin, AgentMemorySelectionChange,
    AgentMemoryStore, AgentSqlMode, ContextMemory, HistoryTools, MemoryInclusion,
    MemoryInclusionChangeValue, MemorySelectionActor, OpenAgentWaitInput, OpenBotDatabase,
    RetrievedAgentMemory, SaveMemoryInput, SharedAgentDataDb, build_memory_context,
    essential_memory_bytes, normalize_memory_site,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use uuid::Uuid;

use crate::agent_lifecycle::{AgentLifecycleService, CreateAgentRequest};
use crate::attention_registry::{
    ApprovalKind, ApprovalRequestInput, AskUiInput, AskUserInput, AttentionRegistry,
};
use crate::browser_service::{BROWSER_TOOL_NAMES, BrowserService};
use crate::channel_service::{ChannelService, ChannelToolExecutionContext};
use crate::drain_scheduler::{
    ActiveTurnOwnerHint, DrainScheduler, InterruptAgentInput, MailboxSendInput, PinActor,
    PinMessageInput, ReactMessageInput, SendChatMessageInput,
};
use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::providers::ProviderRuntimeManager;
use crate::skill_mcp_service::{McpGateway, SkillService};
use crate::tool_catalog::{
    OPENBOT_BROWSER_NAMESPACE, OPENBOT_NAMESPACE, ToolCatalog, ToolCatalogList, ToolDescribeResult,
    ToolSearchResult, decode_arguments_text, is_discovery_tool, null_structured_item_paths,
    qualified_tool_name, repair_json_text, with_schema_types,
};
use crate::vault_service::VaultService;

/// Execution context for any tool invocation routed through [`OpenBotToolRouter`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallContext {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_routine_id: Option<String>,
    /// `true` when invoked directly by the human user/CLI rather than autonomously by an agent turn.
    #[serde(default)]
    pub user_initiated: bool,
}

impl ToolCallContext {
    pub fn for_agent(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            ..Self::default()
        }
    }

    pub fn for_channel(
        agent_id: impl Into<String>,
        channel_id: impl Into<String>,
        task_id: Option<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            channel_id: Some(channel_id.into()),
            task_id,
            ..Self::default()
        }
    }
}

/// Canonical result returned by [`OpenBotToolRouter::call_tool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallOutcome {
    pub tool_name: String,
    pub repaired_json: bool,
    pub result: JsonValue,
}

/// Input for [`OpenBotToolRouter::memory_put`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPutInput {
    pub agent_id: String,
    pub text: String,
    #[serde(default)]
    pub memory_id: Option<String>,
    #[serde(default)]
    pub inclusion: Option<MemoryInclusion>,
    #[serde(default)]
    pub essential: Option<bool>,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub user_controlled: bool,
    #[serde(default)]
    pub source_turn_id: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
}

/// Result of [`OpenBotToolRouter::memory_put`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPutOutcome {
    pub memory: AgentMemory,
    pub inclusion: MemoryInclusion,
    pub user_controlled: bool,
    pub revision: i64,
    pub essential_bytes: usize,
    pub budget_bytes: usize,
    pub stored_count: usize,
}

/// Input for [`OpenBotToolRouter::memory_search`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchInput {
    pub agent_id: String,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub channel_id: Option<String>,
}

/// Result of [`OpenBotToolRouter::memory_search`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchOutcome {
    pub memories: Vec<RetrievedAgentMemory>,
    pub site_memories: Vec<AgentMemory>,
    pub has_more: bool,
    pub essential_bytes: usize,
    pub budget_bytes: usize,
}

/// Extension hook allowing higher-level platform subsystems (workflows, routines, CRM, sites,
/// marketplace, HTML widgets, bridges) to attach handlers to [`OpenBotToolRouter`] cleanly.
pub trait ExternalToolDelegate: Send + Sync {
    fn handle_tool(
        &self,
        ctx: &ToolCallContext,
        qualified_tool: &str,
        args: &JsonValue,
    ) -> RuntimeResult<Option<JsonValue>>;
}

/// Unified runtime tool dispatcher combining all M3 services and storage engines.
pub struct OpenBotToolRouter {
    database: OpenBotDatabase,
    shared_agent_data: SharedAgentDataDb,
    catalog: Arc<ToolCatalog>,
    providers: Arc<ProviderRuntimeManager>,
    agents: Arc<AgentLifecycleService>,
    scheduler: Arc<DrainScheduler>,
    attention: Arc<AttentionRegistry>,
    channels: Arc<ChannelService>,
    skills: Arc<SkillService>,
    mcp: Arc<McpGateway>,
    vault: Arc<VaultService>,
    browser: Arc<BrowserService>,
    external_delegate: RwLock<Option<Arc<dyn ExternalToolDelegate>>>,
}

impl OpenBotToolRouter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: OpenBotDatabase,
        shared_agent_data: SharedAgentDataDb,
        catalog: Arc<ToolCatalog>,
        providers: Arc<ProviderRuntimeManager>,
        agents: Arc<AgentLifecycleService>,
        scheduler: Arc<DrainScheduler>,
        attention: Arc<AttentionRegistry>,
        channels: Arc<ChannelService>,
        skills: Arc<SkillService>,
        mcp: Arc<McpGateway>,
        vault: Arc<VaultService>,
        browser: Arc<BrowserService>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            shared_agent_data,
            catalog,
            providers,
            agents,
            scheduler,
            attention,
            channels,
            skills,
            mcp,
            vault,
            browser,
            external_delegate: RwLock::new(None),
        })
    }

    pub fn catalog(&self) -> &Arc<ToolCatalog> {
        &self.catalog
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn shared_agent_data(&self) -> &SharedAgentDataDb {
        &self.shared_agent_data
    }

    pub fn set_external_delegate(&self, delegate: Option<Arc<dyn ExternalToolDelegate>>) {
        *self.external_delegate.write() = delegate;
    }

    pub fn list_tools(
        &self,
        provider: Option<AgentProviderId>,
        channel_turn: bool,
    ) -> ToolCatalogList {
        self.catalog.list_tools(provider, channel_turn)
    }

    pub fn search_tools(&self, query: &str, limit: usize) -> RuntimeResult<ToolSearchResult> {
        self.catalog.search_tools(query, limit)
    }

    pub fn describe_tools(&self, names: &[String]) -> RuntimeResult<ToolDescribeResult> {
        self.catalog.describe_tools(names)
    }

    /// Decodes, validates (including recursive meta-tool refusal, JSON escape repair, and `[null]`
    /// structured item guards), and dispatches a tool call to its backing runtime service.
    pub fn call_tool(
        &self,
        ctx: &ToolCallContext,
        tool_name: &str,
        raw_or_json_args: &str,
        is_raw: bool,
    ) -> RuntimeResult<ToolCallOutcome> {
        let qualified = canonicalize_router_tool_name(tool_name);
        let (mut parsed_args, repaired_json) =
            self.decode_router_call_args(&qualified, raw_or_json_args, is_raw)?;

        if let Some(tab_id) = ctx.tab_id.as_deref().filter(|s| !s.is_empty())
            && qualified.starts_with("openbot_browser.")
            && let Some(obj) = parsed_args.as_object_mut()
            && !obj.contains_key("tabId")
            && !obj.contains_key("tab_id")
        {
            obj.insert("tabId".to_string(), JsonValue::String(tab_id.to_string()));
        }

        let result = self.dispatch_tool(ctx, &qualified, &parsed_args)?;
        Ok(ToolCallOutcome {
            tool_name: qualified,
            repaired_json,
            result,
        })
    }

    fn decode_router_call_args(
        &self,
        qualified: &str,
        raw_or_json_args: &str,
        is_raw: bool,
    ) -> RuntimeResult<(JsonValue, bool)> {
        if qualified.is_empty()
            || is_discovery_tool(qualified)
            || self.catalog.get(qualified).is_some()
        {
            return self
                .catalog
                .decode_and_validate_call_args(qualified, raw_or_json_args, is_raw);
        }
        if let Some(catalog_alias) = router_alias_catalog_target(qualified) {
            return self.catalog.decode_and_validate_call_args(
                catalog_alias,
                raw_or_json_args,
                is_raw,
            );
        }
        if let Some(ext_schema) = router_extension_schema(qualified) {
            let (parsed_obj, repaired_json) =
                decode_json_object_with_repair(qualified, raw_or_json_args, is_raw)?;
            let typed = with_schema_types(&parsed_obj, &ext_schema);
            let null_paths = null_structured_item_paths(&typed, &ext_schema);
            if !null_paths.is_empty() {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid null value in structured item for '{qualified}' at path(s): {}",
                    null_paths.join(", ")
                )));
            }
            return Ok((typed, repaired_json));
        }
        self.catalog
            .decode_and_validate_call_args(qualified, raw_or_json_args, is_raw)
    }

    /// Convenience helper accepting a pre-parsed `JsonValue` while still enforcing full
    /// `ToolCatalog::decode_and_validate_call_args` guards.
    pub fn call_tool_value(
        &self,
        ctx: &ToolCallContext,
        tool_name: &str,
        args: JsonValue,
    ) -> RuntimeResult<ToolCallOutcome> {
        let encoded = serde_json::to_string(&args)?;
        self.call_tool(ctx, tool_name, &encoded, false)
    }

    fn dispatch_tool(
        &self,
        ctx: &ToolCallContext,
        qualified: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        if let Some(browser_tool) = qualified.strip_prefix(&format!("{OPENBOT_BROWSER_NAMESPACE}."))
        {
            return self
                .browser
                .execute_tool(browser_tool, &ctx.agent_id, args.clone());
        }

        let short = qualified.strip_prefix("openbot.").unwrap_or(qualified);

        match short {
            // --- 3-Tier Memory Tools ---
            "remember" | "memory_save" | "memory_put" | "essential_memory"
            | "forget_memory" | "memory_delete" | "search_memories" | "memory_search"
            | "list_memories" | "set_memory_inclusion" => {
                self.exec_memory_tool(ctx, short, args)
            }

            // --- Agent Conversation History ---
            "history_search" | "history_read" => self.exec_history_tool(ctx, short, args),

            // --- SharedAgentDataDb Supervised SQL Tools ---
            "list_tables" | "sql_list_tables" | "query_data" | "sql_query"
            | "execute_data" | "sql_exec" | "agent_data_exec" | "delete_table" => {
                self.exec_data_tool(ctx, short, args)
            }

            // --- Multi-Bot Channel Tools (15+ tools & aliases) ---
            "channel_history"
            | "channel_assign"
            | "channel_transfer"
            | "channel_result"
            | "channel_reply"
            | "channel_cancel"
            | "channel_remember"
            | "channel_forget_memory"
            | "channel_forget"
            | "channel_add_member"
            | "channel_remove_member"
            | "channel_set_instructions"
            | "channel_set_topic"
            | "channel_list_routines"
            | "channel_create_routine"
            | "channel_update_routine"
            | "channel_delete_routine"
            | "channel_continue"
            | "channel_reassign"
            | "channel_status"
            | "channel_list_members"
            | "channel_send"
            | "channel_await"
            | "list_channels"
            | "create_channel"
            | "update_channel"
            | "send_to_channel" => {
                let ch_ctx = ChannelToolExecutionContext {
                    channel_id: ctx
                        .channel_id
                        .clone()
                        .or_else(|| json_opt_str(args, &["channelId", "channel_id"]))
                        .unwrap_or_default(),
                    agent_id: ctx.agent_id.clone(),
                    task_id: ctx.task_id.clone(),
                    turn_id: ctx.turn_id.clone(),
                    call_id: ctx.call_id.clone(),
                    origin_routine_id: ctx.origin_routine_id.clone(),
                };
                self.channels.execute_channel_tool(&ch_ctx, short, args)
            }

            // --- Skill Tools ---
            "create_skill"
            | "revise_skill"
            | "list_local_skills"
            | "read_local_skill"
            | "install_local_skill"
            | "set_skill_enabled"
            | "uninstall_skill" => self.skills.execute_skill_tool(&ctx.agent_id, short, args),

            // --- MCP Server Management Tools ---
            "list_mcp_servers"
            | "save_mcp_server"
            | "test_mcp_server"
            | "set_mcp_server_enabled"
            | "remove_mcp_server" => self.mcp.execute_mcp_server_tool(&ctx.agent_id, short, args),

            // --- Vault API Key Tools ---
            "request_api_key" | "list_api_keys" | "save_api_key" | "remove_api_key" => {
                self.exec_vault_tool(ctx, short, args)
            }

            // --- Agent Lifecycle & Messaging / Mailbox / DM Tools ---
            "list_agents" | "list_models" | "read_agent" | "create_agent"
            | "update_profile" | "update_agents" | "delete_agent" | "send_message"
            | "interrupt_agent" | "start_new_chat" | "set_browser_profile"
            | "set_commit_co_author" => self.exec_agent_tool(ctx, short, args),

            // --- Side Tasks & Event Waits ---
            "start_task" | "message_task" | "close_task" | "await_event"
            | "close_wait" | "list_waits" => self.exec_task_tool(ctx, short, args),

            // --- Social (Reactions & Pins) ---
            "react_to_user_message" | "pin_message" => self.exec_social_tool(ctx, short, args),

            // --- Interactive Attention (`ask_user` / `ask_ui`) ---
            "ask_user" | "ask_ui" | "acknowledge_request" | "request_location" => {
                self.exec_ui_tool(ctx, short, args)
            }

            // --- Routine Quiet Markers ---
            "routine_no_update" | "routine_report_failure" => {
                self.exec_routine_marker_tool(ctx, short, args)
            }

            _ => {
                if let Some(delegate) = self.external_delegate.read().clone()
                    && let Some(handled) = delegate.handle_tool(ctx, qualified, args)?
                {
                    return Ok(handled);
                }
                Err(OpenBotRuntimeError::NotFound(format!(
                    "Tool '{qualified}' is not registered in the active OpenBot runtime."
                )))
            }
        }
    }

    fn is_isolated_channel(&self, channel_id: Option<&str>) -> RuntimeResult<bool> {
        let Some(cid) = channel_id.filter(|s| !s.trim().is_empty()) else {
            return Ok(false);
        };
        let channel = self.channels.get(cid)?;
        Ok(channel.draft.isolated == Some(true))
    }

    fn assert_not_isolated_channel(&self, channel_id: Option<&str>) -> RuntimeResult<()> {
        if self.is_isolated_channel(channel_id)? {
            return Err(OpenBotRuntimeError::Forbidden(
                "Isolated channels cannot read or modify private agent memory; use channel_remember or channel_forget_memory instead.".to_string(),
            ));
        }
        Ok(())
    }
}

impl OpenBotToolRouter {
    /// Saves or updates an agent memory across the 3-tier memory system (`<= 4,096 B` essential,
    /// FTS5 `bm25` + CJK searchable, and `site` domain hints), enforcing channel isolation.
    pub fn memory_put(&self, input: MemoryPutInput) -> RuntimeResult<MemoryPutOutcome> {
        self.assert_not_isolated_channel(input.channel_id.as_deref())?;
        if input.agent_id.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "agentId is required for memory operations.".to_string(),
            ));
        }
        let trimmed = input.text.trim();
        if trimmed.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Memory text is required.".to_string(),
            ));
        }
        if utf16_len(trimmed) > INPUT_LIMITS.agent_memory_text {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Memory text exceeds the {}-character limit.",
                INPUT_LIMITS.agent_memory_text
            )));
        }
        let normalized_site = match &input.site {
            Some(raw) if !raw.trim().is_empty() => {
                let Some(norm) = normalize_memory_site(raw) else {
                    return Err(OpenBotRuntimeError::Validation(
                        "site must be a valid host name or URL.".to_string(),
                    ));
                };
                Some(norm)
            }
            _ => None,
        };

        let requested_inclusion = input.inclusion.or_else(|| {
            input.essential.map(|flag| {
                if flag {
                    MemoryInclusion::Essential
                } else {
                    MemoryInclusion::Searchable
                }
            })
        });

        let store = self.database.agent_memory_store();
        let actor = if input.user_controlled {
            MemorySelectionActor::User
        } else {
            MemorySelectionActor::Agent
        };

        self.database.with_tx(|_| {
            let saved: AgentMemory = if input.user_controlled && normalized_site.is_none() {
                match input.memory_id.as_deref() {
                    Some(mid) => store.update_manual(&input.agent_id, mid, trimmed)?,
                    None => store.create_manual(&input.agent_id, trimmed)?,
                }
            } else {
                let turn_id = input
                    .source_turn_id
                    .clone()
                    .unwrap_or_else(|| format!("turn:{}", Uuid::new_v4()));
                let entry = openbot_storage::MemoryStore::new(self.database.clone()).save_base(
                    &input.agent_id,
                    SaveMemoryInput {
                        memory_id: input.memory_id.clone(),
                        text: trimmed.to_string(),
                        origin: if input.user_controlled {
                            AgentMemoryOrigin::Manual
                        } else {
                            AgentMemoryOrigin::Automatic
                        },
                        source_turn_id: if input.user_controlled {
                            None
                        } else {
                            Some(turn_id)
                        },
                        command_id: None,
                        uncapped: false,
                        site: normalized_site.clone().map(Some),
                    },
                )?;
                AgentMemory {
                    entry,
                    agent_id: input.agent_id.clone(),
                }
            };

            let current_sel = store
                .get_selection(&input.agent_id, &saved.entry.id)?
                .ok_or_else(|| {
                    openbot_storage::StorageError::NotFound(
                        "Memory selection metadata missing.".to_string(),
                    )
                })?;

            let target_inclusion = requested_inclusion.unwrap_or(current_sel.inclusion);
            let needs_update = target_inclusion != current_sel.inclusion
                || (input.user_controlled && !current_sel.user_controlled);

            let state = if needs_update {
                let change_val = match target_inclusion {
                    MemoryInclusion::Essential => MemoryInclusionChangeValue::Essential,
                    MemoryInclusion::Searchable => MemoryInclusionChangeValue::Searchable,
                };
                store.set_inclusions(
                    &input.agent_id,
                    &[AgentMemorySelectionChange {
                        memory_id: saved.entry.id.clone(),
                        inclusion: change_val,
                        expected_revision: current_sel.revision,
                    }],
                    actor,
                )?
            } else {
                store.selection_state(&input.agent_id)?
            };

            let final_sel = state
                .selections
                .iter()
                .find(|s| s.memory_id == saved.entry.id)
                .cloned()
                .unwrap_or(current_sel);
            let stored_count = store.count(&input.agent_id)?;

            Ok(MemoryPutOutcome {
                memory: saved,
                inclusion: final_sel.inclusion,
                user_controlled: final_sel.user_controlled,
                revision: final_sel.revision,
                essential_bytes: state.used_bytes,
                budget_bytes: state.budget_bytes,
                stored_count,
            })
        }).map_err(OpenBotRuntimeError::from)
    }

    /// Searches long-term FTS5 `bm25` + CJK memories and/or recalls `site`-scoped host memories.
    pub fn memory_search(&self, input: MemorySearchInput) -> RuntimeResult<MemorySearchOutcome> {
        self.assert_not_isolated_channel(input.channel_id.as_deref())?;
        if input.agent_id.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "agentId is required for memory_search.".to_string(),
            ));
        }
        let store = self.database.agent_memory_store();
        let limit = input.limit.unwrap_or(10).clamp(1, 10);

        let (memories, has_more) = match input.query.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => {
                let res = store.search(&input.agent_id, q, limit)?;
                (res.memories, res.has_more)
            }
            _ => (Vec::new(), false),
        };

        let site_memories = match input.host.as_deref().map(str::trim) {
            Some(h) if !h.is_empty() => {
                store.for_host(&input.agent_id, h, openbot_storage::SITE_MEMORY_LIMIT)?
            }
            _ => Vec::new(),
        };

        if input.query.as_deref().map(str::trim).unwrap_or("").is_empty()
            && input.host.as_deref().map(str::trim).unwrap_or("").is_empty()
        {
            return Err(OpenBotRuntimeError::Validation(
                "Provide a search query or host for memory_search.".to_string(),
            ));
        }

        let state = store.selection_state(&input.agent_id)?;
        Ok(MemorySearchOutcome {
            memories,
            site_memories,
            has_more,
            essential_bytes: state.used_bytes,
            budget_bytes: state.budget_bytes,
        })
    }

    /// Deletes an agent memory, protecting `user_controlled` entries from autonomous agent deletion.
    pub fn memory_delete(
        &self,
        agent_id: &str,
        memory_id: &str,
        channel_id: Option<&str>,
        user_initiated: bool,
    ) -> RuntimeResult<bool> {
        self.assert_not_isolated_channel(channel_id)?;
        let store = self.database.agent_memory_store();
        if !user_initiated
            && let Some(sel) = store.get_selection(agent_id, memory_id)?
            && sel.user_controlled
        {
            return Err(OpenBotRuntimeError::Forbidden(
                "The user controls this memory selection and only the user can delete it."
                    .to_string(),
            ));
        }
        Ok(store.delete(agent_id, memory_id)?)
    }

    /// Executes a supervised SQL query or write on `SharedAgentDataDb` (`agent-data.db`).
    pub fn agent_data_exec(
        &self,
        agent_id: &str,
        mode: AgentSqlMode,
        sql: &str,
        params: Option<&JsonValue>,
    ) -> RuntimeResult<JsonValue> {
        match mode {
            AgentSqlMode::Read => {
                let res = self.shared_agent_data.query_checked(agent_id, sql, params)?;
                Ok(serde_json::to_value(res)?)
            }
            AgentSqlMode::Write => {
                let res = self
                    .shared_agent_data
                    .execute_checked(agent_id, sql, params)?;
                let tables = self.shared_agent_data.list_tables()?;
                Ok(json!({
                    "changes": res.changes,
                    "lastInsertRowid": res.last_insert_rowid,
                    "ownerAgentId": agent_id,
                    "tablesCount": tables.len(),
                }))
            }
        }
    }
}

impl OpenBotToolRouter {
    fn exec_memory_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let isolated_channel = self.is_isolated_channel(ctx.channel_id.as_deref())?;
        if isolated_channel
            && matches!(
                short,
                "remember" | "memory_save" | "memory_put" | "essential_memory"
            )
        {
            if json_opt_str(args, &["memoryId", "memory_id"]).is_some() {
                return Err(OpenBotRuntimeError::Forbidden(
                    "This channel is isolated, so a memory from it cannot change your own memories. Save the corrected fact without memoryId: it becomes a channel memory."
                        .to_string(),
                ));
            }
            let text = json_req_str(args, &["text", "memory"], "text")?;
            let channel_id = ctx.channel_id.clone().unwrap_or_default();
            let ch_ctx = ChannelToolExecutionContext {
                channel_id,
                agent_id: ctx.agent_id.clone(),
                task_id: ctx.task_id.clone(),
                turn_id: ctx.turn_id.clone(),
                call_id: ctx.call_id.clone(),
                origin_routine_id: ctx.origin_routine_id.clone(),
            };
            let result = self.channels.execute_channel_tool(
                &ch_ctx,
                "channel_remember",
                &json!({ "text": text }),
            )?;
            if result.get("accepted").and_then(JsonValue::as_bool) == Some(true) {
                return Ok(json!({
                    "status": "saved",
                    "savedAs": "channel memory",
                    "note": "This channel is isolated: the members of this channel see the memory here, and your other chats do not.",
                    "memory": result.get("memory").cloned().unwrap_or(JsonValue::Null),
                }));
            }
            return Ok(result);
        }
        if isolated_channel {
            return Err(OpenBotRuntimeError::Forbidden(
                "Isolated channels cannot read or modify private agent memory; use channel_remember or channel_forget_memory instead.".to_string(),
            ));
        }
        let store: AgentMemoryStore = self.database.agent_memory_store();
        match short {
            "remember" | "memory_save" | "memory_put" | "essential_memory" => {
                let text = json_req_str(args, &["text", "memory"], "text")?;
                let memory_id = json_opt_str(args, &["memoryId", "memory_id"]);
                let site = json_opt_str(args, &["site", "host"]);
                let inclusion = if short == "essential_memory" {
                    Some(MemoryInclusion::Essential)
                } else {
                    match json_opt_str(args, &["inclusion"]).as_deref() {
                        Some("essential") => Some(MemoryInclusion::Essential),
                        Some("searchable") => Some(MemoryInclusion::Searchable),
                        Some(other) => {
                            return Err(OpenBotRuntimeError::Validation(format!(
                                "Invalid memory inclusion '{other}'; expected 'essential' or 'searchable'."
                            )));
                        }
                        None => None,
                    }
                };
                let essential = args.get("essential").and_then(JsonValue::as_bool);
                let user_controlled = ctx.user_initiated
                    || args
                        .get("userControlled")
                        .or_else(|| args.get("user_controlled"))
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false);
                let outcome = self.memory_put(MemoryPutInput {
                    agent_id: ctx.agent_id.clone(),
                    text,
                    memory_id,
                    inclusion,
                    essential,
                    site,
                    user_controlled,
                    source_turn_id: ctx.turn_id.clone(),
                    channel_id: ctx.channel_id.clone(),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "search_memories" | "memory_search" => {
                let query = json_opt_str(args, &["query", "q"]);
                let host = json_opt_str(args, &["host", "site"]);
                let limit = args
                    .get("limit")
                    .and_then(JsonValue::as_u64)
                    .map(|n| n as usize);
                let outcome = self.memory_search(MemorySearchInput {
                    agent_id: ctx.agent_id.clone(),
                    query,
                    host,
                    limit,
                    channel_id: ctx.channel_id.clone(),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "list_memories" => {
                let after = json_opt_str(args, &["after", "cursor"]);
                let page = store.list_page(&ctx.agent_id, after.as_deref())?;
                let state = store.selection_state(&ctx.agent_id)?;
                let essentials = store.essential_for(&ctx.agent_id)?;
                let context = build_memory_context(
                    &essentials,
                    Some(store.limit()),
                    Some(store.count(&ctx.agent_id)?),
                    Some(AGENT_MEMORY_CONTEXT_BUDGET_BYTES),
                );
                let essential_ctx: Vec<ContextMemory> =
                    essentials.iter().map(ContextMemory::from).collect();
                Ok(json!({
                    "memories": page.memories,
                    "hasMore": page.has_more,
                    "nextCursor": page.next_cursor,
                    "essentialBytes": essential_memory_bytes(&essential_ctx),
                    "usedBytes": state.used_bytes,
                    "budgetBytes": state.budget_bytes,
                    "storedCount": context.stored_count,
                }))
            }
            "set_memory_inclusion" => {
                let raw_changes = args
                    .get("changes")
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "Provide a 'changes' array for set_memory_inclusion.".to_string(),
                        )
                    })?;
                let mut changes = Vec::with_capacity(raw_changes.len());
                for item in raw_changes {
                    let parsed: AgentMemorySelectionChange = serde_json::from_value(item.clone())?;
                    changes.push(parsed);
                }
                let actor = if ctx.user_initiated {
                    MemorySelectionActor::User
                } else {
                    MemorySelectionActor::Agent
                };
                let state = store.set_inclusions(&ctx.agent_id, &changes, actor)?;
                Ok(serde_json::to_value(state)?)
            }
            "forget_memory" | "memory_delete" => {
                let memory_id = json_req_str(args, &["memoryId", "memory_id", "id"], "memoryId")?;
                let deleted = self.memory_delete(
                    &ctx.agent_id,
                    &memory_id,
                    ctx.channel_id.as_deref(),
                    ctx.user_initiated,
                )?;
                Ok(json!({ "deleted": deleted, "memoryId": memory_id }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported memory tool '{short}'."
            ))),
        }
    }

    fn exec_history_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let history_tools = HistoryTools::new(self.database.clone());
        let thread_id = match ctx.thread_id.as_deref().filter(|s| !s.is_empty()) {
            Some(tid) => tid.to_string(),
            None => history_tools
                .queries()
                .main_thread_id(&ctx.agent_id)?
                .ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(
                        "No active thread found for this agent.".to_string(),
                    )
                })?,
        };
        let out = match short {
            "history_search" => history_tools.history_search(&ctx.agent_id, &thread_id, Some(args))?,
            "history_read" => history_tools.history_read(&ctx.agent_id, &thread_id, Some(args))?,
            _ => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Unsupported history tool '{short}'."
                )));
            }
        };
        Ok(out)
    }

    fn exec_data_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        match short {
            "list_tables" | "sql_list_tables" => {
                let tables = self.shared_agent_data.list_tables()?;
                Ok(json!({ "tables": tables, "count": tables.len() }))
            }
            "query_data" | "sql_query" => {
                let sql = json_req_str(args, &["sql"], "sql")?;
                self.agent_data_exec(&ctx.agent_id, AgentSqlMode::Read, &sql, args.get("params"))
            }
            "execute_data" | "sql_exec" => {
                let sql = json_req_str(args, &["sql"], "sql")?;
                self.agent_data_exec(&ctx.agent_id, AgentSqlMode::Write, &sql, args.get("params"))
            }
            "agent_data_exec" => {
                let sql = json_req_str(args, &["sql"], "sql")?;
                let mode = match json_opt_str(args, &["mode"]).as_deref() {
                    Some("read") => AgentSqlMode::Read,
                    Some("write") | None => AgentSqlMode::Write,
                    Some(other) => {
                        return Err(OpenBotRuntimeError::Validation(format!(
                            "Invalid SQL mode '{other}'; expected 'read' or 'write'."
                        )));
                    }
                };
                self.agent_data_exec(&ctx.agent_id, mode, &sql, args.get("params"))
            }
            "delete_table" => {
                let name = json_req_str(args, &["name", "table"], "name")?;
                if ctx.user_initiated {
                    self.shared_agent_data.remove_table_as_user(&name)?;
                    Ok(json!({ "deleted": true, "table": name }))
                } else {
                    let removed = self.shared_agent_data.remove_table(&ctx.agent_id, &name)?;
                    Ok(json!({ "deleted": true, "table": removed }))
                }
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported SQL data tool '{short}'."
            ))),
        }
    }

    fn exec_vault_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        match short {
            "list_api_keys" => {
                let keys = self.vault.list_api_keys()?;
                Ok(json!({ "keys": keys }))
            }
            "save_api_key" => {
                let name = json_req_str(args, &["name"], "name")?;
                let value = json_req_str(args, &["key", "value", "secret"], "key")?;
                let saved = self
                    .vault
                    .save_api_key_file(&name, &value, Some(&ctx.agent_id))?;
                Ok(serde_json::to_value(saved)?)
            }
            "remove_api_key" => {
                let name = json_req_str(args, &["name"], "name")?;
                let removed = self.vault.remove_api_key(&name)?;
                Ok(json!({ "removed": removed, "name": name }))
            }
            "request_api_key" => {
                let name = json_req_str(args, &["name"], "name")?;
                let reason = json_opt_str(args, &["reason", "description"]);
                let thread_id = ctx
                    .thread_id
                    .clone()
                    .unwrap_or_else(|| format!("thread:{}", ctx.agent_id));
                let rec = self.attention.request_approval(ApprovalRequestInput {
                    agent_id: ctx.agent_id.clone(),
                    thread_id,
                    turn_id: ctx.turn_id.clone(),
                    kind: ApprovalKind::Tool,
                    action: format!("request_api_key:{name}"),
                    title: format!("Provide API key {name}"),
                    detail: reason.clone(),
                    command: None,
                    cwd: None,
                    reason,
                    owner_only: true,
                    ttl_ms: None,
                })?;
                Ok(serde_json::to_value(rec)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported vault tool '{short}'."
            ))),
        }
    }
}

impl OpenBotToolRouter {
    fn exec_agent_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        match short {
            "list_agents" => {
                let agents = self.agents.list_agents();
                Ok(json!({ "agents": agents }))
            }
            "list_models" => {
                let harnesses: Vec<JsonValue> = crate::providers::all_provider_harnesses()
                    .into_iter()
                    .map(|h| {
                        json!({
                            "id": h.id,
                            "displayName": h.descriptor.display_name,
                            "protocol": h.protocol,
                            "defaultModel": h.default_model,
                            "concurrentTurns": h.descriptor.concurrent_turns,
                            "supportsEphemeralToolless": h.supports_ephemeral_toolless,
                        })
                    })
                    .collect();
                let custom_providers = self.providers.list_custom_providers();
                Ok(json!({
                    "providers": harnesses,
                    "customProviders": custom_providers,
                }))
            }
            "read_agent" => {
                let id = json_opt_str(args, &["agentId", "agent_id", "id"])
                    .unwrap_or_else(|| ctx.agent_id.clone());
                let agent = self.agents.require_agent(&id)?;
                Ok(serde_json::to_value(agent)?)
            }
            "create_agent" => {
                let _ = json_req_str(args, &["name"], "name")?;
                let mut map = args.as_object().cloned().unwrap_or_else(JsonMap::new);
                if !map.contains_key("initialMessage") && !map.contains_key("initial_message") {
                    if let Some(first_task) = map
                        .remove("firstTask")
                        .or_else(|| map.remove("first_task"))
                        .filter(JsonValue::is_string)
                    {
                        map.insert("initialMessage".to_string(), first_task);
                    }
                }
                for key in ["title", "description", "initialMessage"] {
                    if map.get(key).is_some_and(JsonValue::is_null) {
                        map.remove(key);
                    }
                }
                let mut req: CreateAgentRequest = serde_json::from_value(JsonValue::Object(map))?;
                if !ctx.user_initiated
                    && let Some(creator) = self.agents.get_agent(&ctx.agent_id)
                {
                    if creator.access == Some(AgentAccess::Workspace) {
                        if req.access == Some(AgentAccess::Full) {
                            return Err(OpenBotRuntimeError::Forbidden(
                                "An agent with workspace access cannot create an agent with full access."
                                    .to_string(),
                            ));
                        }
                        req.access = Some(AgentAccess::Workspace);
                    }
                    if !creator.computer_use_enabled() {
                        if req.computer_use == Some(true) {
                            return Err(OpenBotRuntimeError::Forbidden(
                                "An agent without computerUse cannot enable computerUse on a new agent."
                                    .to_string(),
                            ));
                        }
                        req.computer_use = Some(false);
                    }
                    if !creator.allow_automation.unwrap_or(false)
                        && req.allow_automation == Some(true)
                    {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "Only the user can enable allowAutomation on a new agent.".to_string(),
                        ));
                    }
                }
                let created = self.agents.create_agent_full(req)?;
                Ok(serde_json::to_value(created)?)
            }
            "update_profile" => {
                let mut map = args.as_object().cloned().unwrap_or_else(JsonMap::new);
                map.entry("agentId".to_string())
                    .or_insert_with(|| JsonValue::String(ctx.agent_id.clone()));
                let input = parse_update_agent_input(&map, &ctx.agent_id)?;
                let initiator = (!ctx.user_initiated).then_some(ctx.agent_id.as_str());
                let updated = self.agents.update_agent(&input, initiator)?;
                Ok(serde_json::to_value(updated)?)
            }
            "update_agents" => {
                let initiator = (!ctx.user_initiated).then_some(ctx.agent_id.as_str());
                if let Some(updates) = args.get("updates").and_then(JsonValue::as_array) {
                    let mut results = Vec::with_capacity(updates.len());
                    for item in updates {
                        let map = item.as_object().ok_or_else(|| {
                            OpenBotRuntimeError::Validation(
                                "Each agent update entry must be a JSON object.".to_string(),
                            )
                        })?;
                        let input = parse_update_agent_input(map, &ctx.agent_id)?;
                        results.push(self.agents.update_agent(&input, initiator)?);
                    }
                    return Ok(json!({ "agents": results }));
                }

                let raw_ids = json_str_list(args, &["agentIds", "agent_ids"]);
                let agent_ids = dedup_ordered(raw_ids);
                if agent_ids.is_empty() {
                    return Err(OpenBotRuntimeError::Validation(
                        "Provide non-empty 'agentIds' or 'updates' array for update_agents."
                            .to_string(),
                    ));
                }
                let map = args.as_object().ok_or_else(|| {
                    OpenBotRuntimeError::Validation(
                        "update_agents arguments must be a JSON object.".to_string(),
                    )
                })?;
                for aid in &agent_ids {
                    let _ = self.agents.require_agent(aid)?;
                }
                let mut results = Vec::with_capacity(agent_ids.len());
                for aid in agent_ids {
                    let mut input = parse_update_agent_input(map, &aid)?;
                    input.agent_id = aid;
                    results.push(self.agents.update_agent(&input, initiator)?);
                }
                Ok(json!({ "agents": results }))
            }
            "delete_agent" => {
                let mut ids = dedup_ordered(json_str_list(args, &["agentIds", "agent_ids"]));
                if ids.is_empty()
                    && let Some(single) = json_opt_str(args, &["agentId", "agent_id", "id"])
                {
                    ids.push(single);
                }
                if ids.is_empty() {
                    return Err(OpenBotRuntimeError::Validation(
                        "Missing required argument 'agentIds' (or 'agentId').".to_string(),
                    ));
                }
                if !ctx.user_initiated {
                    if ids.iter().any(|id| id == &ctx.agent_id) {
                        return Err(OpenBotRuntimeError::Forbidden(
                            "An agent cannot delete itself.".to_string(),
                        ));
                    }
                    let thread_id = ctx
                        .thread_id
                        .clone()
                        .unwrap_or_else(|| format!("thread:{}", ctx.agent_id));
                    let action_ids = ids.join(",");
                    let title = if ids.len() == 1 {
                        format!("Delete agent {}", ids[0])
                    } else {
                        format!("Delete {} agents", ids.len())
                    };
                    let gate = self.attention.request_approval(ApprovalRequestInput {
                        agent_id: ctx.agent_id.clone(),
                        thread_id,
                        turn_id: ctx.turn_id.clone(),
                        kind: ApprovalKind::AgentDelete,
                        action: format!("delete_agent:{action_ids}"),
                        title,
                        detail: Some(
                            "Deleting an agent requires explicit human approval.".to_string(),
                        ),
                        command: None,
                        cwd: None,
                        reason: json_opt_str(args, &["reason"]),
                        owner_only: true,
                        ttl_ms: None,
                    })?;
                    return Err(AttentionRegistry::approval_required_error(&gate));
                }
                let mut deleted_agents = Vec::with_capacity(ids.len());
                let mut deleted_ids = Vec::with_capacity(ids.len());
                for id in ids {
                    if let Some(removed) = self.agents.delete_agent(&id)? {
                        deleted_ids.push(removed.id.clone());
                        deleted_agents.push(removed);
                    }
                }
                let single = deleted_agents.first().cloned();
                Ok(json!({
                    "deleted": !deleted_agents.is_empty(),
                    "deletedCount": deleted_agents.len(),
                    "agent": single,
                    "agents": deleted_agents,
                    "deletedIds": deleted_ids,
                }))
            }
            "send_message" => {
                let recipients = json_str_list(
                    args,
                    &["recipientAgentIds", "recipient_agent_ids", "recipients"],
                );
                let recipients = if recipients.is_empty() {
                    json_opt_str(args, &["recipientAgentId", "recipient_agent_id", "to"])
                        .into_iter()
                        .collect()
                } else {
                    recipients
                };
                let text = json_req_str(args, &["text", "message"], "text")?;
                let expects_reply = args
                    .get("expectsReply")
                    .or_else(|| args.get("expects_reply"))
                    .and_then(JsonValue::as_bool);
                let reply_to_message_id =
                    json_opt_str(args, &["replyToMessageId", "reply_to_message_id"]);
                let idempotency_key =
                    json_opt_str(args, &["idempotencyKey", "idempotency_key"])
                        .or_else(|| ctx.call_id.clone());
                let outcome = self.scheduler.mailbox_send(MailboxSendInput {
                    sender_agent_id: ctx.agent_id.clone(),
                    recipient_agent_ids: recipients,
                    text,
                    expects_reply,
                    reply_to_message_id,
                    sender_task_id: ctx.task_id.clone(),
                    requester_ids: Vec::new(),
                    idempotency_key,
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "interrupt_agent" => {
                let target_agent_id = json_req_str(
                    args,
                    &["targetAgentId", "target_agent_id", "agentId"],
                    "targetAgentId",
                )?;
                let reason = json_opt_str(args, &["reason"]);
                let owner_hint = json_opt_str(args, &["activeTurnOwner", "active_turn_owner"])
                    .and_then(|raw| ActiveTurnOwnerHint::parse(&raw));
                let outcome = self.scheduler.interrupt_agent(InterruptAgentInput {
                    caller_agent_id: ctx.agent_id.clone(),
                    target_agent_id,
                    reason,
                    active_turn_owner: owner_hint,
                    requester_ids: Vec::new(),
                    idempotency_key: ctx.call_id.clone(),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "start_new_chat" => {
                let agent = self.agents.require_agent(&ctx.agent_id)?;
                if !ctx.user_initiated && agent.allow_new_chat != Some(true) {
                    return Err(OpenBotRuntimeError::Forbidden(
                        "start_new_chat is disabled for this agent.".to_string(),
                    ));
                }
                let reason = json_opt_str(args, &["reason"]).unwrap_or_else(|| "reset".to_string());
                let outcome = self.scheduler.compact_context(
                    &ctx.agent_id,
                    ctx.task_id.as_deref(),
                    None,
                    crate::context_compactor::CompactionReason::Manual,
                )?;
                Ok(json!({ "reset": true, "reason": reason, "compaction": outcome }))
            }
            "set_browser_profile" => {
                let profile_id = json_opt_str(args, &["profileId", "profile_id"]);
                let agent_id = json_opt_str(args, &["agentId", "agent_id"])
                    .unwrap_or_else(|| ctx.agent_id.clone());
                let store = self.database.browser_profile_store();
                if let Some(pid) = &profile_id
                    && store.get(pid)?.is_none()
                {
                    return Err(OpenBotRuntimeError::NotFound(format!(
                        "Browser profile '{pid}' does not exist."
                    )));
                }
                if !ctx.user_initiated {
                    let thread_id = ctx
                        .thread_id
                        .clone()
                        .unwrap_or_else(|| format!("thread:{}", ctx.agent_id));
                    let gate = self.attention.request_approval(ApprovalRequestInput {
                        agent_id: ctx.agent_id.clone(),
                        thread_id,
                        turn_id: ctx.turn_id.clone(),
                        kind: ApprovalKind::BrowserProfile,
                        action: format!(
                            "set_browser_profile:{}",
                            profile_id.as_deref().unwrap_or("default")
                        ),
                        title: "Change agent browser profile".to_string(),
                        detail: Some(format!(
                            "Assign browser profile {:?} to agent {agent_id}.",
                            profile_id
                        )),
                        command: None,
                        cwd: None,
                        reason: json_opt_str(args, &["reason"]),
                        owner_only: true,
                        ttl_ms: None,
                    })?;
                    return Ok(serde_json::to_value(gate)?);
                }
                Ok(json!({ "agentId": agent_id, "browserProfileId": profile_id }))
            }
            "set_commit_co_author" => {
                let enabled = args
                    .get("enabled")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(true);
                let thread_id = ctx
                    .thread_id
                    .clone()
                    .unwrap_or_else(|| format!("thread:{}", ctx.agent_id));
                let gate = self.attention.request_approval(ApprovalRequestInput {
                    agent_id: ctx.agent_id.clone(),
                    thread_id,
                    turn_id: ctx.turn_id.clone(),
                    kind: ApprovalKind::CommitCoAuthor,
                    action: format!("set_commit_co_author:{enabled}"),
                    title: format!("Set commit co-author to {enabled}"),
                    detail: None,
                    command: None,
                    cwd: None,
                    reason: json_opt_str(args, &["reason"]),
                    owner_only: true,
                    ttl_ms: None,
                })?;
                Ok(serde_json::to_value(gate)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported agent tool '{short}'."
            ))),
        }
    }
}

impl OpenBotToolRouter {
    fn exec_task_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        match short {
            "start_task" => {
                let title = json_req_str(args, &["title", "name"], "title")?;
                let prompt = json_opt_str(args, &["brief", "prompt", "text", "instruction"]);
                let origin = match json_opt_str(args, &["origin"]).as_deref() {
                    Some("person") | Some("user") => AgentTaskOrigin::User,
                    _ => AgentTaskOrigin::Agent,
                };
                let task = self.scheduler.start_side_task(
                    &ctx.agent_id,
                    &title,
                    origin,
                    prompt.as_deref(),
                )?;
                Ok(serde_json::to_value(task)?)
            }
            "message_task" => {
                let task_id = json_req_str(args, &["taskId", "task_id"], "taskId")?;
                let text = json_req_str(args, &["text", "message", "prompt"], "text")?;
                let outcome = self.scheduler.send_message(SendChatMessageInput {
                    agent_id: ctx.agent_id.clone(),
                    text,
                    task_id: Some(task_id),
                    ..SendChatMessageInput::default()
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "close_task" => {
                let task_id = json_req_str(args, &["taskId", "task_id"], "taskId")?;
                self.scheduler.close_side_task(&ctx.agent_id, &task_id)?;
                Ok(json!({ "closed": true, "taskId": task_id }))
            }
            "await_event" => {
                let note = json_req_str(args, &["note", "reason", "event"], "note")?;
                let request_text = json_opt_str(args, &["requestText", "request_text"])
                    .unwrap_or_else(|| note.clone());
                let request_message_id = json_opt_str(args, &["requestMessageId", "deliveryId"])
                    .unwrap_or_else(|| format!("wait-req:{}", Uuid::new_v4()));
                let routine_id = json_opt_str(args, &["routineId", "routine_id"])
                    .or_else(|| ctx.origin_routine_id.clone());
                let wait = self.database.agent_wait_store().open(OpenAgentWaitInput {
                    agent_id: ctx.agent_id.clone(),
                    request_message_id,
                    request_text,
                    note,
                    routine_id,
                })?;
                Ok(serde_json::to_value(wait)?)
            }
            "close_wait" => {
                let wait_id = json_req_str(args, &["waitId", "wait_id", "id"], "waitId")?;
                let owner_id = json_opt_str(args, &["agentId", "agent_id"])
                    .unwrap_or_else(|| ctx.agent_id.clone());
                let closed = self
                    .database
                    .agent_wait_store()
                    .delete(&owner_id, &wait_id)?;
                Ok(json!({ "closed": closed, "waitId": wait_id }))
            }
            "list_waits" => {
                let owner_id = json_opt_str(args, &["agentId", "agent_id"])
                    .unwrap_or_else(|| ctx.agent_id.clone());
                let waits = self.database.agent_wait_store().list(&owner_id)?;
                Ok(json!({ "waits": waits, "count": waits.len() }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported task tool '{short}'."
            ))),
        }
    }

    fn exec_social_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        match short {
            "react_to_user_message" => {
                let message_id = json_req_str(args, &["messageId", "message_id"], "messageId")?;
                let emoji = json_req_str(args, &["emoji"], "emoji")?;
                let remove = args
                    .get("remove")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
                let actor = if ctx.user_initiated {
                    ConversationReactionActor::User
                } else {
                    ConversationReactionActor::Agent {
                        agent_id: ctx.agent_id.clone(),
                    }
                };
                let outcome = self.scheduler.react_message(ReactMessageInput {
                    agent_id: ctx.agent_id.clone(),
                    task_id: ctx.task_id.clone(),
                    channel_id: ctx
                        .channel_id
                        .clone()
                        .or_else(|| json_opt_str(args, &["channelId", "channel_id"])),
                    message_id,
                    emoji,
                    actor,
                    remove,
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "pin_message" => {
                let message_id = match json_opt_str(args, &["messageId", "message_id"]) {
                    Some(id) => id,
                    None => {
                        if let Some(quote) = json_opt_str(args, &["quote"]) {
                            let snap = self.scheduler.history(
                                &ctx.agent_id,
                                ctx.task_id.as_deref(),
                                None,
                            )?;
                            snap.messages
                                .iter()
                                .rev()
                                .find(|m| m.text.contains(&quote))
                                .map(|m| m.id.clone())
                                .ok_or_else(|| {
                                    OpenBotRuntimeError::NotFound(format!(
                                        "No message in thread matches quote '{quote}'."
                                    ))
                                })?
                        } else {
                            return Err(OpenBotRuntimeError::Validation(
                                "Missing required string argument 'messageId' (or 'quote')."
                                    .to_string(),
                            ));
                        }
                    }
                };
                let unpin = args
                    .get("unpin")
                    .and_then(JsonValue::as_bool)
                    .or_else(|| {
                        args.get("pinned")
                            .and_then(JsonValue::as_bool)
                            .map(|pinned| !pinned)
                    })
                    .unwrap_or(false);
                let actor = if ctx.user_initiated {
                    PinActor::Person { sender: None }
                } else {
                    PinActor::Agent
                };
                let outcome = self.scheduler.pin_message(PinMessageInput {
                    agent_id: ctx.agent_id.clone(),
                    task_id: ctx.task_id.clone(),
                    channel_id: ctx
                        .channel_id
                        .clone()
                        .or_else(|| json_opt_str(args, &["channelId", "channel_id"])),
                    message_id,
                    actor,
                    unpin,
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported social tool '{short}'."
            ))),
        }
    }

    fn exec_ui_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let thread_id = ctx
            .thread_id
            .clone()
            .or_else(|| {
                self.database
                    .history_queries()
                    .main_thread_id(&ctx.agent_id)
                    .ok()
                    .flatten()
            })
            .unwrap_or_else(|| format!("thread:{}", ctx.agent_id));
        match short {
            "ask_user" => {
                let raw_q = args.get("questions").cloned().ok_or_else(|| {
                    OpenBotRuntimeError::Validation("questions array is required.".to_string())
                })?;
                let questions: Vec<AgentPromptQuestion> = serde_json::from_value(raw_q)?;
                let rec = self.attention.ask_user(AskUserInput {
                    agent_id: ctx.agent_id.clone(),
                    thread_id,
                    turn_id: ctx.turn_id.clone(),
                    questions,
                    ttl_ms: args.get("ttlMs").and_then(JsonValue::as_u64),
                })?;
                Ok(serde_json::to_value(rec)?)
            }
            "ask_ui" => {
                let raw_spec = args
                    .get("block")
                    .or_else(|| args.get("spec"))
                    .cloned()
                    .unwrap_or_else(|| args.clone());
                let spec: UiBlockSpec = serde_json::from_value(raw_spec)?;
                let rec = self.attention.ask_ui(AskUiInput {
                    agent_id: ctx.agent_id.clone(),
                    thread_id,
                    turn_id: ctx.turn_id.clone(),
                    block_id: json_opt_str(args, &["blockId", "block_id"]),
                    spec,
                    ttl_ms: args.get("ttlMs").and_then(JsonValue::as_u64),
                })?;
                Ok(serde_json::to_value(rec)?)
            }
            "acknowledge_request" => {
                let text = json_req_str(args, &["text", "message"], "text")?;
                Ok(json!({ "acknowledged": true, "text": text }))
            }
            "request_location" => {
                let reason = json_opt_str(args, &["reason"]);
                let gate = self.attention.request_approval(ApprovalRequestInput {
                    agent_id: ctx.agent_id.clone(),
                    thread_id,
                    turn_id: ctx.turn_id.clone(),
                    kind: ApprovalKind::Permissions,
                    action: "request_location".to_string(),
                    title: "Share approximate location".to_string(),
                    detail: reason.clone(),
                    command: None,
                    cwd: None,
                    reason,
                    owner_only: false,
                    ttl_ms: None,
                })?;
                Ok(serde_json::to_value(gate)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported UI tool '{short}'."
            ))),
        }
    }

    fn exec_routine_marker_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        match short {
            "routine_no_update" => {
                let reason = json_opt_str(args, &["reason", "summary", "note"])
                    .unwrap_or_else(|| "Nothing new to report.".to_string());
                Ok(json!({
                    "accepted": true,
                    "status": "quiet_suppressed",
                    "notified": false,
                    "quietItemType": ROUTINE_QUIET_ANSWER_ITEM_TYPE,
                    "routineId": ctx.origin_routine_id,
                    "reason": reason,
                }))
            }
            "routine_report_failure" => {
                let reason = json_req_str(args, &["reason", "error", "message"], "reason")?;
                Ok(json!({
                    "accepted": true,
                    "status": "failed",
                    "notified": true,
                    "routineId": ctx.origin_routine_id,
                    "error": reason,
                }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported routine marker tool '{short}'."
            ))),
        }
    }
}

fn canonicalize_router_tool_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some((ns, tool)) = trimmed.split_once("__")
        && (ns == OPENBOT_NAMESPACE || ns == OPENBOT_BROWSER_NAMESPACE)
    {
        return format!("{ns}.{tool}");
    }
    if trimmed.contains('.') {
        return trimmed.to_string();
    }
    match trimmed {
        "memory_save" | "memory_put" | "essential_memory" | "memory_search" | "memory_delete"
        | "agent_data_exec" | "sql_exec" | "sql_query" | "sql_list_tables" | "channel_forget"
        | "channel_continue" | "channel_reassign" | "channel_status" | "channel_list_members"
        | "channel_send" | "channel_await" | "channel_set_topic" | "workflow_list"
        | "workflow_get" | "workflow_save" | "workflow_delete" | "workflow_run"
        | "marketplace_search" | "marketplace_install" => format!("openbot.{trimmed}"),
        _ => qualified_tool_name(trimmed),
    }
}

fn router_alias_catalog_target(qualified: &str) -> Option<&'static str> {
    match qualified {
        "openbot.memory_save" | "openbot.memory_put" | "openbot.essential_memory" => {
            Some("openbot.remember")
        }
        "openbot.memory_search" => Some("openbot.search_memories"),
        "openbot.memory_delete" => Some("openbot.forget_memory"),
        "openbot.agent_data_exec" | "openbot.sql_exec" => Some("openbot.execute_data"),
        "openbot.sql_query" => Some("openbot.query_data"),
        "openbot.sql_list_tables" => Some("openbot.list_tables"),
        "openbot.channel_forget" => Some("openbot.channel_forget_memory"),
        "openbot.channel_send" => Some("openbot.send_to_channel"),
        "openbot.workflow_list" => Some("openbot.list_workflows"),
        "openbot.workflow_get" => Some("openbot.read_workflow"),
        "openbot.workflow_save" => Some("openbot.edit_workflow"),
        "openbot.workflow_delete" => Some("openbot.delete_workflow"),
        "openbot.workflow_run" => Some("openbot.run_workflow"),
        "openbot.marketplace_search" => Some("openbot.suggest_marketplace_app"),
        "openbot_browser.back" | "openbot_browser.forward" | "openbot_browser.reload" => {
            Some("openbot_browser.navigate")
        }
        "openbot_browser.new_tab" => Some("openbot_browser.open"),
        "openbot_browser.switch_tab" | "openbot_browser.close" => Some("openbot_browser.close_tab"),
        "openbot_browser.double_click" => Some("openbot_browser.click"),
        "openbot_browser.press_key" => Some("openbot_browser.press"),
        "openbot_browser.extract_text" | "openbot_browser.get_links" => {
            Some("openbot_browser.snapshot")
        }
        "openbot_browser.upload_file" => Some("openbot_browser.upload_files"),
        "openbot_browser.release_takeover" => Some("openbot_browser.request_takeover"),
        _ => None,
    }
}

fn router_extension_schema(qualified: &str) -> Option<JsonValue> {
    match qualified {
        "openbot.channel_continue" | "openbot.channel_reassign" => Some(json!({
            "type": "object",
            "properties": {
                "channelId": { "type": "string" },
                "taskId": { "type": "string" },
                "recipientAgentId": { "type": "string" }
            }
        })),
        "openbot.channel_status" | "openbot.channel_list_members" | "openbot.channel_await" => {
            Some(json!({
                "type": "object",
                "properties": {
                    "channelId": { "type": "string" }
                }
            }))
        }
        "openbot.channel_set_topic" => Some(json!({
            "type": "object",
            "properties": {
                "channelId": { "type": "string" },
                "title": { "type": "string" },
                "topic": { "type": "string" }
            }
        })),
        "openbot.marketplace_install" => Some(json!({
            "type": "object",
            "properties": {
                "tab": { "type": "string" },
                "id": { "type": "string" },
                "slug": { "type": "string" }
            }
        })),
        "openbot_browser.console_logs" => Some(json!({
            "type": "object",
            "properties": {
                "tabId": { "type": "string" },
                "level": { "type": "string" },
                "clear": { "type": "boolean" }
            }
        })),
        "openbot_browser.network_log" => Some(json!({
            "type": "object",
            "properties": {
                "tabId": { "type": "string" },
                "clear": { "type": "boolean" }
            }
        })),
        "openbot_browser.download" => Some(json!({
            "type": "object",
            "properties": {
                "tabId": { "type": "string" },
                "url": { "type": "string" },
                "filename": { "type": "string" }
            }
        })),
        "openbot_browser.cookies" => Some(json!({
            "type": "object",
            "properties": {
                "action": { "type": "string" },
                "domain": { "type": "string" },
                "name": { "type": "string" },
                "cookie": { "type": "object", "additionalProperties": true }
            }
        })),
        _ => {
            if let Some(browser_short) =
                qualified.strip_prefix(&format!("{OPENBOT_BROWSER_NAMESPACE}."))
                && BROWSER_TOOL_NAMES.contains(&browser_short)
            {
                return Some(json!({
                    "type": "object",
                    "properties": {
                        "tabId": { "type": "string" }
                    },
                    "additionalProperties": true
                }));
            }
            None
        }
    }
}

fn decode_json_object_with_repair(
    qualified: &str,
    raw_or_json: &str,
    is_raw: bool,
) -> RuntimeResult<(JsonValue, bool)> {
    let trimmed = raw_or_json.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok((JsonValue::Object(JsonMap::new()), false));
    }
    if is_raw {
        let candidate = repair_json_text(raw_or_json);
        if candidate != raw_or_json
            && let Ok(val) = serde_json::from_str::<JsonValue>(&candidate)
            && val.is_object()
        {
            return Ok((val, true));
        }
    }
    if let Ok(val) = serde_json::from_str::<JsonValue>(raw_or_json) {
        return match val {
            JsonValue::Object(map) => Ok((JsonValue::Object(map), false)),
            JsonValue::Null => Ok((JsonValue::Object(JsonMap::new()), false)),
            _ => Err(OpenBotRuntimeError::Validation(
                "tool_call arguments must decode to a JSON object.".to_string(),
            )),
        };
    }
    if let Some((repaired_val, was_repaired)) = decode_arguments_text(raw_or_json) {
        return Ok((repaired_val, was_repaired));
    }
    Err(OpenBotRuntimeError::Validation(format!(
        "Invalid JSON arguments for tool '{qualified}': could not decode object payload."
    )))
}

fn dedup_ordered(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn parse_update_agent_input(
    map: &JsonMap<String, JsonValue>,
    default_agent_id: &str,
) -> RuntimeResult<UpdateAgentInput> {
    let agent_id = map
        .get("agentId")
        .or_else(|| map.get("agent_id"))
        .or_else(|| map.get("id"))
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(default_agent_id)
        .to_string();

    let access = match map.get("access").and_then(JsonValue::as_str) {
        Some("full") => Some(AgentAccess::Full),
        Some("workspace") => Some(AgentAccess::Workspace),
        Some(other) => {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid agent access '{other}'; expected 'full' or 'workspace'."
            )));
        }
        None => None,
    };

    let provider = match map.get("provider") {
        Some(JsonValue::Null) | None => None,
        Some(v) => Some(serde_json::from_value::<AgentProviderId>(v.clone()).map_err(|_| {
            OpenBotRuntimeError::Validation(format!("Invalid agent provider: {v}"))
        })?),
    };

    let reasoning_effort = match map
        .get("reasoningEffort")
        .or_else(|| map.get("reasoning_effort"))
    {
        Some(JsonValue::Null) | None => None,
        Some(v) => Some(
            serde_json::from_value::<AgentReasoningEffort>(v.clone()).map_err(|_| {
                OpenBotRuntimeError::Validation(format!("Invalid reasoningEffort: {v}"))
            })?,
        ),
    };

    let avatar_hue = match map.get("avatarHue").or_else(|| map.get("avatar_hue")) {
        None => None,
        Some(JsonValue::Null) => Some(None),
        Some(v) => Some(Some(
            serde_json::from_value::<AvatarHue>(v.clone()).map_err(|_| {
                OpenBotRuntimeError::Validation(format!("Invalid avatarHue: {v}"))
            })?,
        )),
    };

    let backup_models = match map.get("backupModels").or_else(|| map.get("backup_models")) {
        None => None,
        Some(JsonValue::Null) => Some(None),
        Some(v) => Some(Some(
            serde_json::from_value::<AgentBackupModels>(v.clone()).map_err(|_| {
                OpenBotRuntimeError::Validation(format!("Invalid backupModels: {v}"))
            })?,
        )),
    };

    let allow_automation = map
        .get("allowAutomation")
        .or_else(|| map.get("allow_automation"))
        .map(|v| v.as_bool());

    Ok(UpdateAgentInput {
        agent_id,
        name: map.get("name").and_then(JsonValue::as_str).map(str::to_string),
        title: map.get("title").and_then(JsonValue::as_str).map(str::to_string),
        description: map
            .get("description")
            .and_then(JsonValue::as_str)
            .map(str::to_string),
        notifications: map.get("notifications").and_then(JsonValue::as_bool),
        provider,
        model: map.get("model").and_then(JsonValue::as_str).map(str::to_string),
        reasoning_effort,
        access,
        computer_use: map
            .get("computerUse")
            .or_else(|| map.get("computer_use"))
            .and_then(JsonValue::as_bool),
        memory: map.get("memory").and_then(JsonValue::as_bool),
        allow_automation,
        fast_mode: map
            .get("fastMode")
            .or_else(|| map.get("fast_mode"))
            .and_then(JsonValue::as_bool),
        ultrafast: map.get("ultrafast").and_then(JsonValue::as_bool),
        avatar_seed: map
            .get("avatarSeed")
            .or_else(|| map.get("avatar_seed"))
            .and_then(JsonValue::as_str)
            .map(str::to_string),
        avatar_hue,
        backup_models,
        ..UpdateAgentInput::default()
    })
}

fn json_opt_str(args: &JsonValue, keys: &[&str]) -> Option<String> {
    let obj = args.as_object()?;
    for key in keys {
        if let Some(s) = obj.get(*key).and_then(JsonValue::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn json_req_str(args: &JsonValue, keys: &[&str], label: &str) -> RuntimeResult<String> {
    json_opt_str(args, keys)
        .ok_or_else(|| OpenBotRuntimeError::Validation(format!("Missing required string argument '{label}'.")))
}

fn json_str_list(args: &JsonValue, keys: &[&str]) -> Vec<String> {
    let Some(obj) = args.as_object() else {
        return Vec::new();
    };
    for key in keys {
        if let Some(arr) = obj.get(*key).and_then(JsonValue::as_array) {
            return arr
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}
