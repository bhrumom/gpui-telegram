//! OpenBot SQLite WAL (`v69`) CQRS event-sourced database, domain stores,
//! atomic JSON preference stores, and authorizer-guarded `SharedAgentDataDb`.

pub mod agent_store;
pub mod channel_store;
pub mod crm_store;
pub mod database;
pub mod error;
pub mod history_queries;
pub mod json_preference_store;
pub mod mailbox_store;
pub mod memory_store;
pub mod redact;
pub mod routine_store;
pub mod schema_v69;
pub mod shared_agent_data;
pub mod skill_mcp_browser_store;
pub mod team_chat_store;
pub mod thread_store;
pub mod usage_store;
pub mod workflow_store;

pub use agent_store::{
    AGENT_PREVIEW_MAX_LENGTH, AgentDuplicationMarker, AgentModelChange, AgentModelRuntime,
    AgentRoster, AgentStore, CLAUDE_FAST_MODE_RESET_COMMAND_ID, DEFAULT_AGENT_MODEL,
    DEFAULT_AGENT_PROVIDER, DEFAULT_REASONING_EFFORT, DuplicateAgentResult,
    ExtendedUpdateAgentInput, ResolvedAgentAvatar, UnclaimedThread, agent_avatar_url,
    duplicate_agent_name, duplication_command_id, duplication_profile_signature, file_fingerprint,
    is_generated_agent_id, is_valid_agent_id, legacy_agent_id, parse_agent_avatar_url,
    stable_thread_id, workspace_metadata_fingerprint,
};
pub use channel_store::{
    ChannelAgentWork, ChannelChange, ChannelExecutionThread, ChannelOlderExtent, ChannelStore,
    ChannelUpdateChanges, MCP_SIGN_IN_ITEM_TYPE_PREFIX, MemberFoldersCallback,
    SETTING_LINK_ITEM_TYPE_PREFIX, SHOWN_MESSAGE_SQL, UNREAD_COUNT_SQL, changes_model_content,
    deleted_channel_message, is_deleted_channel_message, markdown_preview_text, preview_text,
};
pub use crm_store::{
    AGENT_PENDING_PROPOSALS_LIMIT, CRM_ACTIVITY_PAGE_SIZE, CrmActivityPage, CrmActivityPageInput,
    CrmAgentChange, CrmAgentCustomerFields, CrmAgentFindInput, CrmAgentPaymentFilter,
    CrmAgentPaymentInput, CrmAgentPaymentPatch, CrmAgentPaymentQueryState, CrmAgentPaymentsInput,
    CrmAgentPlanInput, CrmAgentPlanPatch, CrmAgentProposeInput, CrmAgentRecord, CrmAgentSplitPart,
    CrmAgentView, CrmCustomerPage, CrmCustomerPageInput, CrmCustomerRecord, CrmCustomerText,
    CrmDetailsMap, CrmPaymentPage, CrmPaymentPageInput, CrmPaymentRecord, CrmProposeInput,
    CrmSourceStore, CrmStatusRecord, CrmStore, CrmWrite, DEFAULT_TOTAL_PENDING_PROPOSALS_LIMIT,
    InsertCrmSourceInput, SourceBatch, SourceCustomer, SourcePayment, SourcePlan, SourceStatePatch,
    add_crm_months, crm_currency_exponent, decode_crm_number_cursor, decode_crm_text_cursor,
    encode_crm_number_cursor, encode_crm_text_cursor, format_crm_amount, local_crm_date_from_ms,
    new_crm_id, normalize_crm_search, parse_crm_amount,
};
pub use database::{
    COLLAPSE_WHITESPACE_FUNCTION, DispatchReceipt, FileDeletionDrainReport, LOWERCASE_FUNCTION,
    OpenBotDatabase, OrchestrationEventInput, PendingFileDeletion, StoredOrchestrationEvent,
    THREAD_COMPACTION_BATCH_BYTES, THREAD_COMPACTION_BATCH_EVENTS, THREAD_COMPACTION_MAX_RESTARTS,
    ThreadEventCompactionBatch, ThreadEventCompactor, collapse_unicode_whitespace,
    merge_replayed_live_message, now_rfc3339_millis, register_openbot_scalar_functions,
    scoped_command_id,
};
pub use error::{StorageError, StorageResult};
pub use history_queries::{
    AgentHistoryPage, AgentHistoryQueries, AgentHistoryRow, AgentHistorySelection,
    CapturedStepsLookupFn, CustomRedactorFn, HISTORY_UNAVAILABLE_MSG, HistoryAnsweredBy,
    HistoryAttachmentInfo, HistoryEntry, HistoryPagePayload, HistorySearchAllPayload,
    HistoryThreadSummary, HistoryThreadsPayload, HistoryTools, MAIN_CHAT_TASK_ID, NO_MATCH_HINT,
    ORDER_KEY_COLUMNS, ORDER_KEY_DESC, ORDERED_THREAD_MESSAGES, READ_CURSOR_HINT,
    READ_RESULT_CHARACTERS, REPLY_EXCERPT_CHARACTERS, REQUEST_EXCERPT_CHARACTERS,
    SEARCH_ALL_CHARACTERS, SEARCH_ALL_THREADS, SEARCH_SCAN_ROWS, StoredAgentThreadSummary,
    THREADS_RESULT_CHARACTERS, ThreadListCursorPosition, VisualPageTextFn, excerpt_text,
    format_trace_work_steps, history_cursor_thread, is_history_browse, parse_search_terms,
    search_match, window_bound, wrap_openbot_tool_result,
};
pub use json_preference_store::{
    JsonPreferenceStore, OPENBOT_PREFERENCE_FILES, PassthroughSecretCipher, PreferenceFileKind,
    PreferenceFileSpec, PreferenceStore, SealedKeyStore, SecretCipher, VersionedJsonFileState,
    VersionedJsonLoad, VersionedJsonStore, find_preference_file_spec, is_empty_stored_file,
    parse_stored_json, preference_files, resolve_preference_file, set_aside_damaged_file,
    strip_utf8_bom, versioned_json_fallback_path, write_file_atomically,
    write_json_file_atomically,
};
pub use mailbox_store::{
    AUTO_RESULT_KEY_PREFIX, AwaitedThreadRequest, CHANNEL_LANE_PREFIX,
    CHANNEL_REQUEST_RETURN_KEY_PREFIX, ChannelRequestReturnsTracker, DeliveryContext, EnqueueInput,
    FAILED_REQUEST_KEY_PREFIX, InterruptRefusal, MAILBOX_CONVERSATION_MESSAGE_LIMIT,
    MAILBOX_METADATA_ROW, MESSAGING_LANE, MailboxPendingFileDeletion, MailboxProjection,
    MailboxProjectionChanges, MailboxProjectionState, MailboxStore, ManagedAttachment,
    MessagingOrigin, MessagingOriginReplyQuote, ParsedTeammateReply, PlanContinuationMarker,
    QueueEditOutcome, QueuedDeliveryReceiptItem, QueuedMessageReceipt, RESTARTED_RESULT_KEY_PREFIX,
    RESTARTED_TURN_KEY_PREFIX, SendAgentMessageInput, StoredAttachment, StoredDraft,
    StoredFinishedEdit, StoredGeneratedAttachment, StoredMailboxDelivery, StoredMailboxMessage,
    StoredReaction, TaskWork, TeammateReplyStatus, UsageLimitContinuation,
    channel_handoff_sender_agent_id, channel_lane, channel_request_return_key,
    format_channel_request_return_text, format_teammate_reply, is_returned_channel_task,
    lane_channel_id, mailbox_reaction_row_key, moved_to_open_task, parse_teammate_reply,
    queue_save_hash, rewrite_attachment_references,
};
pub use memory_store::{
    AGENT_MEMORY_CONTEXT_BUDGET_BYTES, AGENT_MEMORY_LIMIT_CHOICES, AgentMemory,
    AgentMemoryListPage, AgentMemoryOrigin, AgentMemorySearchResult, AgentMemorySelection,
    AgentMemorySelectionChange, AgentMemorySelectionError, AgentMemorySelectionState,
    AgentMemoryStore, ChannelMemory, ChannelMemoryStore, ContextMemory, DEFAULT_AGENT_MEMORY_LIMIT,
    MEMORY_LIST_PAGE_LIMIT, MEMORY_OFF_UPDATE, MEMORY_SELECTION_BATCH_LIMIT,
    MEMORY_SITE_MAX_LENGTH, MemoryContext, MemoryEntry, MemoryInclusion,
    MemoryInclusionChangeValue, MemorySelectionActor, MemorySelectionErrorCode, MemoryStore,
    MemoryTables, MessagingMemoryStore, RetrievedAgentMemory, SITE_MEMORY_LIMIT,
    SaveAutomaticMemory, SaveAutomaticMemoryInput, SaveMemoryInput, StageRememberInput,
    StagedMemoryOutcome, StagedMemoryStatus, StagedTurnMemories, UPSTREAM_V78_MEMORY_BUDGET_BYTES,
    build_memory_context, essential_memory_bytes, extract_search_word_terms, fold_memory_text,
    format_memory_context, initialize_agent_memory_selection, is_unicode_combining_mark,
    is_unspaced_script, matches_unindexed_term, memory_context_update, memory_prompt_line,
    memory_sites_for_host, normalize_memory_site, normalize_memory_text,
    serialize_essential_memories, validate_memory_text,
};
pub use redact::{
    SecretRegistry, UnscannedTextPolicy, contains_secret, is_secret_label, is_secret_name,
    redact_post_text, redact_secrets, redact_text, register_public_values, register_secret_value,
    register_secret_values,
};
pub use routine_store::{
    AGENT_ROUTINE_TABLES, AgentRoutineStore, CHANNEL_ROUTINE_TABLES, ChannelRoutineStore,
    CreateRoutineFlowLinkInput, DueChannelRoutine, DueRoutine, DueRoutineTrigger,
    EXTERNAL_EVENT_END, EXTERNAL_EVENT_START, HIDDEN_AGENT_ROUTINE_PAUSE_COMMAND_ID,
    ImportedRoutineRun, NewRoutineFlowStep, OwnedRoutine, OwnedRoutineRecord, OwnedRoutineRun,
    ROUTINE_RESULT_DELIVERY_LIST_LIMIT, ROUTINE_RESULT_EXPIRY_MS, ReceivedWebhookEvent,
    RoutineFlowConnectProblem, RoutineFlowRun, RoutineFlowStore, RoutineHoldWindow,
    RoutineInputFields, RoutineRecordInput, RoutineRecordTrigger, RoutineRecordTriggerInput,
    RoutineResultOutgoing, RoutineResultPendingRun, RoutineResultStore, RoutineRunStart,
    RoutineStore, RoutineTables, RoutineUpdateFields, SkippedRunEntry, SkippedRunRows,
    WEBHOOK_EVENT_TYPE_MAX_LENGTH, WEBHOOK_FILTERS_LIMIT, WebhookReceiveResult,
    normalize_routine_schedule, routine_flow_connect_problem, routine_flow_depths,
    validate_webhook_trigger, webhook_mismatch, webhook_run_instruction,
};
pub use schema_v69::{
    BASELINE_SCHEMA_VERSION, LATEST_SCHEMA_VERSION, OpenBotMigrationOptions,
    SCHEMA_V69_DDL_SECTIONS, assert_foreign_keys, assert_quick_check, execute_schema_v69_ddl,
    has_existing_schema, latest_applied_version, migrate_openbot_database,
    openbot_database_needs_migration, read_applied_versions, read_foreign_key_violations,
    schema_v69_ddl, validate_applied_versions,
};
pub use shared_agent_data::{
    AGENT_DATABASE_LIMITS, AGENT_DATABASE_SCHEMA_VERSION, AgentCellValue, AgentDatabaseLimits,
    AgentQueryResult, AgentSchemaChange, AgentSqlMode, AgentSqlParam, AgentTableSummary,
    AgentWriteResult, AuthorizerContext, PRIMARY_METADATA_TABLE, SharedAgentDataDb,
    UPSTREAM_METADATA_TABLE, check_agent_parameters, check_agent_sql, install_shared_db_guards,
    schema_change,
};
pub use skill_mcp_browser_store::{
    AgentBrowserProfile, BROWSER_PROFILE_HUES, BROWSER_PROFILE_LIMIT, BROWSER_PROFILE_NAME_MAX,
    BROWSER_PROXY_ADDRESS_MAX, BROWSER_PROXY_LIMIT, BrowserProfileList, BrowserProfileRecord,
    BrowserProfileStore, BrowserProfileSummary, BrowserProxyAddress, BrowserProxyAddressRecord,
    BrowserProxyComputerRecord, BrowserProxyRecord, BrowserProxyRow, BrowserProxyScheme,
    BrowserProxyStore, COMPUTER_USE_MCP_SERVER_ID, COMPUTER_USE_MCP_SERVER_NAME,
    GITHUB_CONNECTOR_MCP_SERVER_ID, GITHUB_CONNECTOR_MCP_SERVER_NAME,
    GITHUB_CONNECTOR_MCP_SERVER_URL, McpConfigErrors, McpKeyValue, McpServerConfig, McpServerStore,
    McpTransport, RESERVED_MCP_SERVER_NAMES, SOCKS_SIGN_IN_MAX_BYTES, browser_profile_name,
    browser_proxy_address_text, create_mcp_server_id, is_browser_profile_id, is_mcp_header_name,
    is_mcp_server_config, is_reserved_mcp_server_name, mcp_config_errors, next_browser_profile_hue,
    normalize_mcp_config, parse_browser_proxy_address,
};
pub use team_chat_store::{
    DIRECT_PAGE_LIMIT, DirectConversationPage, DirectConversationPageAnchor,
    DirectConversationPageInfo, DirectConversationReadState, DirectConversationSnapshot,
    DirectMessage, DirectThreadSummary, InboundFile, MESSAGING_CONTEXT_MESSAGES,
    MESSAGING_LINK_LIST_LIMIT, MessagingAllowedRole, MessagingAllowedSender,
    MessagingConnectionPatch, MessagingConnectionRecord, MessagingContextEntry,
    MessagingHistoryFilter, MessagingHistoryStore, MessagingLink, MessagingLinkInput,
    MessagingPing, MessagingPingInput, MessagingPlatform, MessagingSenderKind, MessagingStore,
    MessagingThreadedEntry, SendDirectMessageInput, TeamChatStore, decode_direct_cursor,
    direct_thread_id, encode_direct_cursor, matches_history_filter, sorted_member_ids,
};
pub use thread_store::{
    AGENT_STARTED_FINISHED_KEPT, AGENT_STARTED_UNFINISHED_TASKS, AgentThreadBranch,
    AgentThreadBranchInput, AgentThreadInput, AgentThreadRecord, AgentThreadStore,
    AgentThreadTitleSource, AgentWait, AgentWaitStore, AppendConversationMessageInput,
    BindProviderSessionInput, ConversationFileSearchPage, ConversationFileSearchResult,
    ConversationMarkerExclusions, ConversationPage, ConversationPageAnchor, ConversationPageInfo,
    ConversationReadStore, ConversationSearchMatch, ConversationSearchPage, MessagePinStore,
    ORDERED_THREAD_MESSAGES_CTE, OpenAgentWaitInput, PAGE_MESSAGE_LIMIT,
    PersistConversationChangesInput, PinRefusalReason, PinnedThreadMessage, ProviderSession,
    ProviderSessionState, ProviderSessionStore, READ_EXCLUDED_ITEM_TYPE_PREFIXES,
    READ_EXCLUDED_ITEM_TYPES, SentMessageStats, StoredThreadSummary, StreamingMessageWrite,
    ThreadListPosition, ThreadStore, UpsertProviderHistoryMessageInput, can_pin_message,
    copy_branch_messages, deleted_thread_message_ids, format_pinned_message,
    is_deleted_thread_message, item_type_counts_as_unread, pin_thread_message_conn,
    pinned_thread_message_count_conn, pinned_thread_messages_conn, prune_conversation_snapshots,
    record_deleted_thread_history, record_deleted_thread_message, refresh_thread_order,
    sort_conversation_messages, supported_conversation_cursor_conn, thread_message_ids,
    thread_message_pinner_conn, thread_messages_containing_conn, unpin_thread_history_conn,
    unpin_thread_message_conn, without_deleted_messages, without_queue_positions,
};
pub use usage_store::{
    AgentAnalytics, AgentAnalyticsInput, AgentUsageStore, AnalyticsAgent, AnalyticsDay,
    AnalyticsModel, AnalyticsProviderDay, AnalyticsSkill, AnalyticsTotals, HostAnalytics,
    HostAnalyticsInput, SkillLoadSample, TurnModelUsage, TurnUsageTotals, UsageCostEstimate,
    UsageSample, UsageTokens, WORKFLOWS_USAGE_ID, analytics_date, estimate_usage_cost, input_price,
    record_usage_message, usage_record_digest,
};
pub use workflow_store::{
    CreateWorkflowInput, DueWorkflowTrigger, EventActivity, GitHubActionRecord, GitHubActionStatus,
    NewGitHubAction, NewGitHubDelivery, NewGitHubRun, NewWorkflowRun, NewWorkflowStep,
    OfflineGitHubTrigger, RoutineWebhookReceiptInput, StoredWorkflowRun, StoredWorkflowStep,
    WEBHOOK_RECEIPT_RETENTION_MS, WORKFLOW_GITHUB_DELIVERY_LIMIT, WORKFLOW_GITHUB_OFFLINE_MERGE_MS,
    WaitingGitHubDelivery, WaitingGitHubEnd, WebhookRoute, WebhookRouteOwner, WebhookRouteStore,
    WorkflowGitHubDelivery, WorkflowGitHubDeliveryOutcome, WorkflowGitHubHistory,
    WorkflowGitHubItem, WorkflowGitHubRunItem, WorkflowGitHubSkipReason, WorkflowGitHubStore,
    WorkflowListGraphEntry, WorkflowMediaOutput, WorkflowOutputDelivery, WorkflowOutputStatus,
    WorkflowRun, WorkflowRunPage, WorkflowRunStore, WorkflowRunWithSteps, WorkflowSaveResult,
    WorkflowStep, WorkflowStepEnd, WorkflowStepSource, WorkflowStore, WorkflowValuesMode,
    WorkflowVersion, WorkflowWebhookReceipt, default_workflow_next_due,
    has_routine_webhook_receipt, insert_routine_webhook_receipt, node_timezone,
    revoke_routine_webhooks, runtime_fields_of_node, webhook_receipt_cutoff, workflow_graph_format,
};

impl OpenBotDatabase {
    pub fn agent_roster(&self) -> AgentRoster {
        AgentRoster::new(self.clone())
    }

    pub fn agent_store(&self) -> StorageResult<AgentStore> {
        AgentStore::open(self.clone(), self.data_dir())
    }

    pub fn agent_store_at(&self, openbot_root: impl AsRef<std::path::Path>) -> StorageResult<AgentStore> {
        AgentStore::open(self.clone(), openbot_root)
    }

    pub fn thread_store(&self) -> ThreadStore {
        ThreadStore::new(self.clone())
    }

    pub fn agent_thread_store(&self) -> AgentThreadStore {
        AgentThreadStore::new(self.clone())
    }

    pub fn message_pin_store(&self) -> MessagePinStore {
        MessagePinStore::new(self.clone())
    }

    pub fn provider_session_store(&self) -> ProviderSessionStore {
        ProviderSessionStore::new(self.clone())
    }

    pub fn agent_wait_store(&self) -> AgentWaitStore {
        AgentWaitStore::new(self.clone())
    }

    pub fn conversation_read_store(&self) -> ConversationReadStore {
        ConversationReadStore::new(self.clone())
    }

    pub fn mailbox_store(&self) -> StorageResult<MailboxStore> {
        MailboxStore::open(self.clone(), self.data_dir().join("Shared"))
    }

    pub fn mailbox_store_at(&self, shared_root: impl AsRef<std::path::Path>) -> StorageResult<MailboxStore> {
        MailboxStore::open(self.clone(), shared_root)
    }

    pub fn team_chat_store(&self) -> TeamChatStore {
        TeamChatStore::new(self.clone())
    }

    pub fn messaging_store(&self) -> MessagingStore {
        MessagingStore::new(self.clone())
    }

    pub fn messaging_history_store(&self) -> MessagingHistoryStore {
        MessagingHistoryStore::new(self.clone())
    }

    pub fn channel_store(&self) -> ChannelStore {
        ChannelStore::new(self.clone())
    }

    pub fn memory_store(&self) -> MemoryStore {
        MemoryStore::new(self.clone())
    }

    pub fn agent_memory_store(&self) -> AgentMemoryStore {
        AgentMemoryStore::new(self.clone())
    }

    pub fn channel_memory_store(&self) -> ChannelMemoryStore {
        ChannelMemoryStore::new(self.clone())
    }

    pub fn messaging_memory_store(&self) -> MessagingMemoryStore {
        MessagingMemoryStore::new(self.clone())
    }

    pub fn history_queries(&self) -> AgentHistoryQueries {
        AgentHistoryQueries::new(self.clone())
    }

    pub fn history_tools(&self) -> HistoryTools {
        HistoryTools::new(self.clone())
    }

    pub fn workflow_store(&self) -> WorkflowStore {
        WorkflowStore::new(self.clone())
    }

    pub fn workflow_run_store(&self) -> WorkflowRunStore {
        WorkflowRunStore::new(self.clone())
    }

    pub fn workflow_github_store(&self) -> WorkflowGitHubStore {
        WorkflowGitHubStore::new(self.clone())
    }

    pub fn webhook_route_store(&self) -> WebhookRouteStore {
        WebhookRouteStore::new(self.clone())
    }

    pub fn agent_routine_store(&self) -> AgentRoutineStore {
        AgentRoutineStore::new(self.clone())
    }

    pub fn channel_routine_store(&self) -> ChannelRoutineStore {
        ChannelRoutineStore::new(self.clone())
    }

    pub fn routine_flow_store(&self) -> RoutineFlowStore {
        RoutineFlowStore::new(self.clone())
    }

    pub fn routine_result_store(&self) -> RoutineResultStore {
        RoutineResultStore::new(self.clone())
    }

    pub fn usage_store(&self) -> AgentUsageStore {
        AgentUsageStore::new(self.clone())
    }

    pub fn crm_store(&self) -> CrmStore {
        CrmStore::new(self.clone())
    }

    pub fn crm_source_store(&self) -> CrmSourceStore {
        CrmSourceStore::new(CrmStore::new(self.clone()))
    }

    pub fn crm_agent_view(&self) -> CrmAgentView {
        CrmAgentView::new(CrmStore::new(self.clone()))
    }

    pub fn browser_profile_store(&self) -> BrowserProfileStore {
        BrowserProfileStore::new(self.clone())
    }

    pub fn browser_proxy_store(&self) -> BrowserProxyStore {
        BrowserProxyStore::new(self.clone())
    }

    pub fn mcp_server_store(&self) -> McpServerStore {
        McpServerStore::new(self.clone())
    }

    pub fn shared_agent_data_db(&self) -> StorageResult<SharedAgentDataDb> {
        SharedAgentDataDb::open(self.data_dir().join("Shared").join("Data").join("agent-data.db"))
    }
}
