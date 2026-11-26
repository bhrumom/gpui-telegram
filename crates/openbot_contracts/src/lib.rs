//! `openbot_contracts` — Pure-Rust Shared Domain Contracts (`OB-F01`..`OB-F12`).
//! 1:1 faithful port of `@openbot/contracts` & `@openbot/brand` pinned to upstream commit `23183891`.

pub mod agents;
pub mod avatar;
pub mod channels;
pub mod conversation;
pub mod crm;
pub mod dynamic_island;
mod emoji_rgi;
pub mod ipc;
pub mod limits;
pub mod mentions;
pub mod providers;
pub mod queue;
pub mod routines;
pub mod tags;
pub mod team_protocol;
pub mod ui_blocks;
pub mod voice;
pub mod workflows;

pub const OPENBOT_CONTRACTS_UPSTREAM_COMMIT: &str = "23183891";

pub use agents::{
    AgentAccess, AgentBackupModels, AgentDefaults, AgentModelChoice, AgentModelOption,
    AgentProfileDraft, AgentReasoningEffort, AgentSummary, AgentTask, AgentTaskAwaitedReply,
    AgentTaskAwaitedState, AgentTaskOrigin, AgentTasksSnapshot, BackupModelSpec, BusyMessageMode,
    ClientAgentModelOption, ClientAgentSummary, CreateAgentInput, LegacyAgentReasoningEffort,
    UpdateAgentInput, agent_welcome_message, is_browser_profile_id, legacy_reasoning_effort,
};
pub use avatar::{
    AvatarColorChoice, AvatarColorPresetId, AvatarHue, AvatarMimeType, AvatarMood, AvatarStyle,
    BloubAvatarProfile, BloubColorId, BloubExpression, BloubParametricState, BloubShape,
    EngineShapeId, PlushState, PlushTransition, ServerPlushColor, ShapeSafeStateId,
    bloub_avatar_profile, compute_agent_avatar_mood, compute_agent_avatar_moods, fnv1a_utf16_32,
    nearest_avatar_hue,
};
pub use channels::{
    Channel, ChannelAssignment, ChannelBrowser, ChannelCommand, ChannelContext, ChannelDraft,
    ChannelHistorySummary, ChannelIcon, ChannelIconName, ChannelMember, ChannelMessage,
    ChannelPage, ChannelPreview, ChannelRoutingDecision, ChannelSummary, ChannelTask,
    ChannelTaskState, ChannelToolCall, CreateChannelToolIcon, CreateChannelToolInput,
    UpdateChannelToolInput, check_channel_reply,
};
pub use conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus, ConversationPlan,
    ConversationQuestionPrompt, ConversationReaction, ConversationReadState, ConversationSnapshot,
    ConversationSubagent, ConversationTurnTrace, PinnedMessage,
};
pub use crm::{
    CRM_LIMITS, CrmApplyResult, CrmChange, CrmConflictReason, CrmCustomer, CrmOverview, CrmPayment,
    CrmPlan, CrmPrivateField, CrmProposal, CrmProposedChange, CrmSettings,
};
pub use dynamic_island::{
    DynamicIslandAction, DynamicIslandApprovalDetails, DynamicIslandApprovalPermissions,
    DynamicIslandFileSystemPermissions, DynamicIslandMode, DynamicIslandPreference,
    DynamicIslandPresentation, DynamicIslandViewState, interaction_locks_presentation,
};
pub use ipc::{
    AgentEvent, AgentIpcRequest, IPC_ENDPOINTS, IPC_NESTED_API_PATHS, IpcEndpointDescriptor,
    IpcEndpointKind, IpcGroupName, ScopedAgentEvent, ScopedRequest, ServerArgument, ServerScope,
};
pub use limits::{
    ATTACHMENT_LIMITS, AVATAR_IMAGE_LIMITS, ContractError, INPUT_LIMITS, InputLimits, RequestId,
    double_option, forbid_field, is_bounded_string, is_identifier, non_null_option, utf16_len,
};
pub use mentions::{
    MentionableAgent, MentionableAgentInfo, NamedChannelAgent, agent_mention_rank,
    fold_mention_text, rank_agent_mentions, resolve_channel_recipients, typed_address,
};
pub use providers::{
    AGENT_PROVIDER_DESCRIPTORS, AGENT_PROVIDERS, AgentProviderDescriptor, AgentProviderId,
    AgentProviderRef, CustomProviderEndpoint, CustomProviderHarness, CustomProviderSummary,
    ForeignProviderId,
};
pub use queue::{
    QUEUE_EDIT_HOLD_MS, QueueDelivery, QueueDeliveryStatus, QueueEditRequest, QueueHold, QueueItem,
    QueueSnapshot,
};
pub use routines::{
    ChannelRoutine, ChannelRoutineRun, EventFilter, EventRoutine, QuietRunPolicy, Routine,
    RoutineDefinition, RoutineFlowCanvas, RoutineFlowRoutine, RoutineFlowRoutineInfo,
    RoutineLimitPolicy, RoutineMissedPolicy, RoutineResultDelivery, RoutineRun, RoutineRunPlace,
    RoutineSchedule, collapse_missed_occurrences, next_routine_occurrence,
    next_valid_routine_occurrence, routine_once_instant, sign_webhook_body,
    validate_routine_schedule, validate_stored_routine_schedule,
    verify_standard_webhook_signature, verify_webhook_signature,
};
pub use tags::{
    ChatTagKind, ChatTagReference, EVERYONE_MENTION, FileQuoteTarget, LeadingAddress,
    chat_tag_references, expand_chat_tag_references, file_quote_id, file_quote_target,
    leading_address, serialize_chat_tag_reference,
};
pub use team_protocol::{
    TEAM_ABSENT_CAPABILITIES_HEADER, TEAM_API_ROUTES, TEAM_APP_VERSION_HEADER,
    TEAM_CAPABILITIES_HEADER, TEAM_CAPABILITY_REGISTRY, TEAM_PROTOCOL_VERSION_HEADER,
    TeamCapabilitiesV10, TeamProtocolRange, TeamProtocolSupport, encode_team_capabilities_v10,
    expand_team_capabilities_v10, highest_common_team_protocol, team_capabilities_for_protocol,
    team_protocol_update_direction, to_current_agent_keys, to_wire_agent_keys,
};
pub use ui_blocks::{
    ConversationUiBlock, UI_BLOCK_LIMITS, UiBlockResponse, UiBlockSpec, UiBlockState,
    UiBlockStatus, validate_ui_block_response,
};
pub use voice::{
    CodexLiveVoice, SpeechModelId, SpeechSettings, VoiceConfig, VoiceLiveEvent, VoiceLiveSession,
    VoiceLiveStart, VoiceLiveStop, VoiceModelId, VoiceServerModels, VoiceServerProbe,
    VoiceServerProbeField, VoiceServerSettings, VoiceSettings, default_voice_settings,
    next_speech_speed,
};
pub use workflows::{
    WORKFLOW_NODE_REGISTRY, WORKFLOW_PORTS, WorkflowBranch, WorkflowDefinition, WorkflowDetail,
    WorkflowEdge, WorkflowFileAsset, WorkflowGraph, WorkflowGraphProblem, WorkflowNode,
    WorkflowNodeKind, WorkflowPortType, WorkflowPortableFile, WorkflowSummary,
    WorkflowTriggerState, workflow_graph_has_unknown_key, workflow_loop_edges,
    workflow_node_has_unknown_value,
};
