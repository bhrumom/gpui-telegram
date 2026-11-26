//! Wire contracts and prefixed `Orca*` DTOs for Orca REST, WebSocket, and Unified RPC v3.
//!
//! Every exported type in this module is prefixed with `Orca` to guarantee zero naming
//! collisions with existing `gpui_telegram_proto` wildcard re-exports (`miniapp_contracts`,
//! `ws`, `api`, `payments`, etc.).

use std::collections::{BTreeMap, HashSet};

use orca_core::bot_ecosystem::{
    BotChatNode, BotImportCandidate, NativeChatBotCard, SealedTeamShareHeldMessage,
    SealedTeamShareOffer, TeamSharePublicIdentity, UsageLimitContinue,
};
use orca_core::bot_groups::{
    BotGroupImage, BotGroupPeerScope, BotGroupReplyMode, BotGroupSyntheticRoot, RemoteBotMember,
};
use orca_core::bot_items::{AskSelection, BotItemActor, BotItemAsk, BotItemStatus};
use orca_core::bot_look_around::Suggestion;
use orca_core::bot_memory::BotMemoryAuthor;
use orca_core::bot_types::{
    AcceptedBotOffer, BotAgentId, BotGitHubLink, BotItemsStale, BotLaunchConfig, BotLinearLink,
    BotLookAroundConfig, BotMascotId, BotMember, BotNotifications, BotOfferAnswerState,
    BotOfferAnswerUpdate, BotPermissionMode, BotSlackLink, BotTidyUpConfig,
};
use orca_core::orchestration_protocol::{
    CallerScope, RpcErrorPayload, RpcRequestEnvelope, RpcResponseEnvelope,
};
use orca_core::triggers_webhooks::{
    AutomationBotTarget, AutomationEventTrigger, OneShotWait, WebhookAdapterId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// 1. Canonical Error & Unified RPC v3 Wire Envelopes
// ---------------------------------------------------------------------------

/// Canonical nested error detail inside [`OrcaErrorEnvelope`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaErrorDetail {
    pub code: String,
    pub message: String,
}

/// Canonical Orca REST error envelope: `{"error": {"code": "<snake_case>", "message": "..."}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaErrorEnvelope {
    pub error: OrcaErrorDetail,
}

impl OrcaErrorEnvelope {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: OrcaErrorDetail {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

/// Typed alias for the canonical Unified RPC v3 request envelope (`POST /api/v1/orca/rpc`).
pub type OrcaRpcRequest = RpcRequestEnvelope;

/// Typed alias for the canonical Unified RPC v3 response envelope.
pub type OrcaRpcResponse = RpcResponseEnvelope;

/// Typed alias for Unified RPC v3 caller scope.
pub type OrcaCallerScope = CallerScope;

/// Typed alias for Unified RPC v3 error payload.
pub type OrcaRpcErrorPayload = RpcErrorPayload;

// ---------------------------------------------------------------------------
// 2. Bot Fleet CRUD & Built-in Templates
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/orca/bots`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaCreateBotRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default, alias = "homePath")]
    pub home_path: Option<String>,
    #[serde(default)]
    pub mascot: Option<BotMascotId>,
    #[serde(default, alias = "avatarColor")]
    pub avatar_color: Option<String>,
    #[serde(default, alias = "avatarOutfit")]
    pub avatar_outfit: Option<String>,
    #[serde(default, alias = "customSoul", alias = "soul")]
    pub custom_soul: Option<String>,
    #[serde(default, alias = "permissionMode")]
    pub permission_mode: Option<BotPermissionMode>,
    #[serde(default)]
    pub launch: Option<BotLaunchConfig>,
    #[serde(default, alias = "folderWorkspaceId")]
    pub folder_workspace_id: Option<String>,
    #[serde(default, alias = "templateId")]
    pub template_id: Option<String>,
    #[serde(default, alias = "firstRun")]
    pub first_run: bool,
    #[serde(default, alias = "itemsStale")]
    pub items_stale: Option<BotItemsStale>,
    #[serde(default, alias = "tidyUp")]
    pub tidy_up: Option<Value>,
    #[serde(default, alias = "lookAround")]
    pub look_around: Option<BotLookAroundConfig>,
}

/// Member removal descriptor inside [`OrcaUpdateBotRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaBotMemberRemovalDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub id: String,
    #[serde(default, alias = "workspaceId", skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// Connector links patch inside [`OrcaUpdateBotRequest`] (deliberately excludes `host_mcp_servers`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaBotConnectorPatchDto {
    #[serde(default)]
    pub slack: Option<Option<BotSlackLink>>,
    #[serde(default, alias = "slackApps")]
    pub slack_apps: Option<Option<Vec<BotSlackLink>>>,
    #[serde(default)]
    pub linear: Option<Option<BotLinearLink>>,
    #[serde(default, alias = "linearWorkspaces")]
    pub linear_workspaces: Option<Option<Vec<BotLinearLink>>>,
    #[serde(default)]
    pub github: Option<Option<BotGitHubLink>>,
    #[serde(default, alias = "slackMirror")]
    pub slack_mirror: Option<Option<bool>>,
    #[serde(default)]
    pub google: Option<Option<bool>>,
    #[serde(default, alias = "googleAccounts")]
    pub google_accounts: Option<Option<Vec<String>>>,
}

/// Request body for `PATCH /api/v1/orca/bots/{bot_id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaUpdateBotRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub paused: Option<bool>,
    #[serde(default, alias = "permissionMode")]
    pub permission_mode: Option<Option<BotPermissionMode>>,
    #[serde(default)]
    pub launch: Option<Option<BotLaunchConfig>>,
    #[serde(default)]
    pub notifications: Option<BotNotifications>,
    #[serde(default, alias = "addMembers")]
    pub add_members: Vec<BotMember>,
    #[serde(default, alias = "removeMember")]
    pub remove_member: Option<OrcaBotMemberRemovalDto>,
    #[serde(default, alias = "removeMembers")]
    pub remove_members: Vec<OrcaBotMemberRemovalDto>,
    #[serde(default)]
    pub connectors: Option<OrcaBotConnectorPatchDto>,
    #[serde(default, alias = "skillProjects")]
    pub skill_projects: Option<Option<Vec<String>>>,
    #[serde(default, alias = "sidebarOrder")]
    pub sidebar_order: Option<Option<u32>>,
    #[serde(default, alias = "chatSessionId")]
    pub chat_session_id: Option<Option<String>>,
    #[serde(default, alias = "lookAround")]
    pub look_around: Option<Option<BotLookAroundConfig>>,
    #[serde(default, alias = "tidyUp")]
    pub tidy_up: Option<Option<BotTidyUpConfig>>,
    #[serde(default, alias = "itemsStale")]
    pub items_stale: Option<Option<BotItemsStale>>,
    #[serde(default, alias = "itemsSeen")]
    pub items_seen: Option<Option<u64>>,
    #[serde(default, alias = "firstRun")]
    pub first_run: Option<bool>,
    #[serde(default, alias = "offerAnswer")]
    pub offer_answer: Option<OrcaBotOfferAnswerUpdateDto>,
    #[serde(default, alias = "acceptOffer")]
    pub accept_offer: Option<AcceptedBotOffer>,
    #[serde(default, alias = "folderWorkspaceId")]
    pub folder_workspace_id: Option<Option<String>>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default, alias = "color", alias = "avatarColor")]
    pub avatar_color: Option<String>,
    #[serde(default, alias = "outfit", alias = "avatarOutfit")]
    pub avatar_outfit: Option<String>,
    #[serde(default, alias = "avatarPngB64")]
    pub avatar_png_b64: Option<String>,
    #[serde(default, alias = "expectedAvatarRevision")]
    pub expected_avatar_revision: Option<u64>,
    #[serde(default, alias = "expectedAvatarRevisionEpoch")]
    pub expected_avatar_revision_epoch: Option<String>,
    #[serde(default, alias = "hostMcpServers")]
    pub host_mcp_servers: Option<bool>,
    #[serde(default, alias = "usageLimitContinue")]
    pub usage_limit_continue: Option<Value>,
}

/// Wire DTO mirroring `orca_core::bot_types::BotOfferAnswerUpdate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OrcaBotOfferAnswerUpdateDto {
    Claim {
        #[serde(alias = "offerId")]
        offer_id: String,
        state: BotOfferAnswerState,
        #[serde(alias = "answeredAt")]
        answered_at: i64,
    },
    Release {
        #[serde(alias = "offerId")]
        offer_id: String,
    },
}

impl From<OrcaBotOfferAnswerUpdateDto> for BotOfferAnswerUpdate {
    fn from(dto: OrcaBotOfferAnswerUpdateDto) -> Self {
        match dto {
            OrcaBotOfferAnswerUpdateDto::Claim {
                offer_id,
                state,
                answered_at,
            } => Self::Claim {
                offer_id,
                state,
                answered_at,
            },
            OrcaBotOfferAnswerUpdateDto::Release { offer_id } => Self::Release { offer_id },
        }
    }
}

/// Response for `DELETE /api/v1/orca/bots/{bot_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaDeleteBotResponse {
    pub deleted: bool,
    pub bot_id: String,
    pub released_folder: String,
}

/// Request body for `POST /api/v1/orca/bots/from-template`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaFromTemplateRequest {
    #[serde(alias = "templateId")]
    pub template_id: String,
    #[serde(default, alias = "customName", alias = "name")]
    pub custom_name: Option<String>,
}

// ---------------------------------------------------------------------------
// 3. Memory Governance & Overnight Tidy-Up
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/orca/bots/{bot_id}/memory`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaMemoryWriteRequest {
    #[serde(default, alias = "content")]
    pub markdown: Option<String>,
    #[serde(default, alias = "appendLine", alias = "fact")]
    pub append_line: Option<String>,
    #[serde(default, alias = "beforeMemoryMd")]
    pub before_memory_md: Option<String>,
    #[serde(default, alias = "turnSource", alias = "source")]
    pub turn_source: Option<String>,
    #[serde(default, alias = "ownerTurnText")]
    pub owner_turn_text: Option<String>,
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default, alias = "allowDeletions")]
    pub allow_deletions: Option<bool>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
    #[serde(default)]
    pub today: Option<String>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/memory/forget`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaMemoryForgetRequest {
    #[serde(alias = "fact")]
    pub query: String,
    #[serde(default)]
    pub by: Option<BotMemoryAuthor>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/memory/undo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaMemoryUndoRequest {
    #[serde(default, alias = "history_id", alias = "rowId", alias = "historyId")]
    pub row_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Schedule/enabled configuration inside [`OrcaTidyUpRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaTidyUpConfigureDto {
    pub enabled: bool,
    #[serde(default = "default_tidy_up_schedule")]
    pub schedule: String,
}

fn default_tidy_up_schedule() -> String {
    "night".to_string()
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/tidy-up`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaTidyUpRequest {
    #[serde(default)]
    pub configure: Option<OrcaTidyUpConfigureDto>,
    #[serde(default)]
    pub run: Option<bool>,
    #[serde(default)]
    pub force: bool,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
    #[serde(default)]
    pub today: Option<String>,
    #[serde(default, alias = "utcOffsetSeconds")]
    pub utc_offset_seconds: Option<i32>,
    #[serde(default, alias = "allowSleepCatchup")]
    pub allow_sleep_catchup: bool,
    #[serde(default)]
    pub chats: Vec<Value>,
    #[serde(default)]
    pub draft: Option<Value>,
    #[serde(default, alias = "rawReply")]
    pub raw_reply: Option<String>,
    #[serde(default, alias = "simulateError")]
    pub simulate_error: Option<String>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/tidy-up/undo`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaTidyUpUndoRequest {
    #[serde(default, alias = "history_id", alias = "rowId", alias = "historyId")]
    pub row_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// 4. Bot Items, Ask Cards, Snooze & Look Around / Up Next
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/orca/bots/{bot_id}/items`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaCreateItemRequest {
    pub title: String,
    #[serde(default)]
    pub actor: Option<BotItemActor>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, alias = "doneMeans")]
    pub done_means: Option<String>,
    #[serde(default)]
    pub status: Option<BotItemStatus>,
    #[serde(default)]
    pub ask: Option<BotItemAsk>,
    #[serde(default, alias = "waitNote", alias = "wait")]
    pub wait_note: Option<String>,
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<String>,
    #[serde(default, alias = "telegramChatId")]
    pub telegram_chat_id: Option<Value>,
    #[serde(default, alias = "groupId")]
    pub group_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default, alias = "madeBy")]
    pub made_by: Option<BotItemActor>,
    #[serde(default)]
    pub links: Vec<Value>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `PATCH /api/v1/orca/items/{item_num}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaUpdateItemRequest {
    #[serde(default)]
    pub actor: Option<BotItemActor>,
    #[serde(default, alias = "expectedVersion")]
    pub expected_version: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default, alias = "doneMeans")]
    pub done_means: Option<Option<String>>,
    #[serde(default)]
    pub status: Option<BotItemStatus>,
    #[serde(default, alias = "waitNote", alias = "wait")]
    pub wait_note: Option<String>,
    #[serde(default, alias = "closeNote")]
    pub close_note: Option<String>,
    #[serde(default, alias = "comment")]
    pub note: Option<String>,
    #[serde(default)]
    pub ask: Option<BotItemAsk>,
    #[serde(default)]
    pub link: Option<Value>,
    #[serde(default, alias = "telegramChatId")]
    pub telegram_chat_id: Option<Value>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/items/{item_num}/comment`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaItemCommentRequest {
    #[serde(default)]
    pub actor: Option<BotItemActor>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default, alias = "questionEvent")]
    pub question_event: Option<u64>,
    #[serde(default)]
    pub selections: Option<Vec<AskSelection>>,
    #[serde(default)]
    pub answers: Option<Value>,
    #[serde(default, alias = "replyText")]
    pub reply_text: Option<String>,
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<Value>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/items/{item_num}/snooze`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaItemSnoozeRequest {
    #[serde(default)]
    pub actor: Option<BotItemActor>,
    #[serde(default, alias = "untilMs")]
    pub until_ms: Option<i64>,
    #[serde(default, alias = "snoozedUntil", alias = "until")]
    pub snoozed_until: Option<Value>,
    #[serde(default)]
    pub unsnooze: bool,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Schedule patch inside [`OrcaLookAroundRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaLookAroundPatchDto {
    #[serde(default, alias = "enabled")]
    pub on: Option<bool>,
    #[serde(default, alias = "schedule")]
    pub every: Option<String>,
    #[serde(default, alias = "weekdaysOnly")]
    pub weekdays_only: Option<bool>,
    #[serde(default)]
    pub focus: Option<Option<String>>,
    #[serde(default, alias = "sinceMs", alias = "since_ms")]
    pub since: Option<Value>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/look-around`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaLookAroundRequest {
    #[serde(default)]
    pub patch: Option<OrcaLookAroundPatchDto>,
    #[serde(default, alias = "enabled")]
    pub on: Option<bool>,
    #[serde(default, alias = "schedule")]
    pub every: Option<String>,
    #[serde(default, alias = "weekdaysOnly")]
    pub weekdays_only: Option<bool>,
    #[serde(default)]
    pub focus: Option<Option<String>>,
    #[serde(default, alias = "sinceMs", alias = "since_ms")]
    pub since: Option<Value>,
    #[serde(default)]
    pub run: Option<bool>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub suggestions: Option<Vec<Suggestion>>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
    #[serde(default, alias = "utcOffsetSeconds")]
    pub utc_offset_seconds: Option<i32>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/up-next`.
/// Supports both action dispatch (`action: "start"|"dismiss"`) and direct suggestion submission
/// (`title`, `why`, `ask`, optional `link`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaUpNextActionRequest {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default, alias = "suggestionId")]
    pub suggestion_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub ask: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// 5. Bot Chats, Reply (`--leave-thread` / `NO_REPLY`), Groups, Access, Hand-Over & Chat Tree
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/orca/bots/{bot_id}/chats`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaCreateBotChatRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/reply`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaBotChatReplyRequest {
    #[serde(default, alias = "chatId", alias = "sessionId")]
    pub chat_id: Option<String>,
    pub text: String,
    #[serde(default, alias = "leaveThread")]
    pub leave_thread: bool,
    #[serde(default, alias = "isForwarded")]
    pub is_forwarded: bool,
    #[serde(default, alias = "isQuotedReply")]
    pub is_quoted_reply: bool,
    #[serde(default, alias = "addressedBotName")]
    pub addressed_bot_name: Option<String>,
    #[serde(default, alias = "otherBotNames")]
    pub other_bot_names: Vec<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/groups`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaCreateGroupRequest {
    pub name: String,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default, alias = "memberBotIds")]
    pub member_bot_ids: Vec<String>,
    #[serde(default, alias = "replyMode")]
    pub reply_mode: Option<BotGroupReplyMode>,
    #[serde(default, alias = "remoteMembers")]
    pub remote_members: Vec<RemoteBotMember>,
}

/// Request body for `POST /api/v1/orca/groups/{group_id}/threads`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaCreateGroupThreadRequest {
    #[serde(alias = "title", alias = "firstPost", alias = "text")]
    pub first_post: String,
}

/// Request body for `POST /api/v1/orca/groups/{group_id}/send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaSendGroupMessageRequest {
    pub text: String,
    #[serde(default, alias = "threadId")]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub sender: Option<Value>,
    #[serde(default, alias = "fromBotId")]
    pub from_bot_id: Option<String>,
    #[serde(default)]
    pub handoff: bool,
    #[serde(default)]
    pub images: Vec<BotGroupImage>,
    #[serde(default)]
    pub requester: Option<Value>,
    #[serde(default, alias = "peerScope")]
    pub peer_scope: Option<BotGroupPeerScope>,
    #[serde(default, alias = "syntheticRoot")]
    pub synthetic_root: Option<BotGroupSyntheticRoot>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/access`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaCreateAccessRequest {
    #[serde(default)]
    pub member: Option<BotMember>,
    #[serde(default, alias = "connector")]
    pub source: Option<String>,
    #[serde(default, alias = "externalUserId", alias = "external_user_id", alias = "userId")]
    pub user_id: Option<String>,
    #[serde(default, alias = "displayName", alias = "display_name")]
    pub name: Option<String>,
    #[serde(default, alias = "connectorWorkspaceId")]
    pub connector_workspace_id: Option<String>,
    #[serde(default, alias = "ttlMs")]
    pub ttl_ms: Option<i64>,
    #[serde(default, alias = "targetHostId")]
    pub target_host_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/access/decide`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaDecideAccessRequest {
    #[serde(default, alias = "requestId")]
    pub request_id: Option<String>,
    #[serde(default)]
    pub approve: Option<bool>,
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default, alias = "redeemToken")]
    pub redeem_token: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/hand-over`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaHandOverRequest {
    #[serde(alias = "workspaceId", alias = "workspacePath", alias = "workspace_path")]
    pub workspace_id: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default, alias = "workspaceName")]
    pub workspace_name: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default, alias = "repoSlug")]
    pub repo_slug: Option<String>,
    #[serde(default, alias = "recentPrompt", alias = "prompt")]
    pub recent_prompt: Option<String>,
    #[serde(default)]
    pub agent: Option<BotAgentId>,
    #[serde(default, alias = "customName")]
    pub custom_name: Option<String>,
    #[serde(default, alias = "expectedInitialParent", alias = "originalParent", alias = "original_parent")]
    pub expected_initial_parent: Option<String>,
    #[serde(default, alias = "simulateNotifyFailure")]
    pub simulate_notify_failure: Option<Value>,
    #[serde(default, alias = "simulateConcurrentReparent", alias = "concurrentParent", alias = "concurrent_parent")]
    pub simulate_concurrent_reparent: Option<String>,
    #[serde(default, alias = "simulateRestoreFailure")]
    pub simulate_restore_failure: Option<String>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/chat-tree`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaChatTreeRequest {
    #[serde(default)]
    pub nodes: Vec<BotChatNode>,
    #[serde(default, alias = "usageLimit")]
    pub usage_limit: Option<UsageLimitContinue>,
    #[serde(default)]
    pub cards: Vec<NativeChatBotCard>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// 6. Packages, E2EE Team Share & Cross-App Import
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/orca/bots/{bot_id}/package/export`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaPackageExportRequest {
    #[serde(default, alias = "includeMemory")]
    pub include_memory: Option<bool>,
    #[serde(default, alias = "includeMemoryIndex")]
    pub include_memory_index: Option<bool>,
    #[serde(default, alias = "includeMemoryTopics")]
    pub include_memory_topics: Option<bool>,
    #[serde(default, alias = "includeSkills")]
    pub include_skills: Option<bool>,
    #[serde(default, alias = "includeAvatarPng", alias = "include_avatar")]
    pub include_avatar_png: Option<bool>,
    #[serde(default, alias = "connectorsHint")]
    pub connectors_hint: Vec<String>,
}

/// Request body for `POST /api/v1/orca/bots/package/import`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaPackageImportRequest {
    #[serde(alias = "zipB64", alias = "package")]
    pub zip_b64: String,
    #[serde(default, alias = "customName", alias = "name")]
    pub custom_name: Option<String>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/share/seal`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaShareSealRequest {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default, alias = "recipientIdentity")]
    pub recipient_identity: Option<TeamSharePublicIdentity>,
    #[serde(default, alias = "offerId")]
    pub offer_id: Option<String>,
    #[serde(default, alias = "shareId")]
    pub share_id: Option<String>,
    #[serde(default, alias = "hostKey")]
    pub host_key: Option<String>,
    #[serde(default, alias = "botId")]
    pub bot_id: Option<String>,
    #[serde(default, alias = "expiresAtMs")]
    pub expires_at_ms: Option<i64>,
    #[serde(default, alias = "relayUrl")]
    pub relay_url: Option<String>,
    #[serde(default, alias = "senderName")]
    pub sender_name: Option<String>,
    #[serde(default, alias = "clientMessageId")]
    pub client_message_id: Option<String>,
    #[serde(default, alias = "plaintext")]
    pub text: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/bots/share/open`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaShareOpenRequest {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub offer: Option<SealedTeamShareOffer>,
    #[serde(default, alias = "heldMessage")]
    pub held_message: Option<SealedTeamShareHeldMessage>,
    #[serde(default)]
    pub envelope: Option<Value>,
    #[serde(default, alias = "hostKey")]
    pub host_key: Option<String>,
    #[serde(default, alias = "receiverKey")]
    pub receiver_key: Option<String>,
    #[serde(default, alias = "expectedSenderEd25519B64")]
    pub expected_sender_ed25519_b64: Option<String>,
    #[serde(default, alias = "senderIdentity")]
    pub sender_identity: Option<TeamSharePublicIdentity>,
    #[serde(default, alias = "expectedShareId", alias = "shareId")]
    pub expected_share_id: Option<String>,
    #[serde(default, alias = "expectedBotId", alias = "botId")]
    pub expected_bot_id: Option<String>,
    #[serde(default, alias = "clientMessageId")]
    pub client_message_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Explicit foreign-app discovery directories for [`OrcaImportDiscoverRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaImportRootsDto {
    #[serde(default, alias = "openclawDirs")]
    pub openclaw_dirs: Vec<String>,
    #[serde(default, alias = "hermesDirs")]
    pub hermes_dirs: Vec<String>,
    #[serde(default, alias = "openbotDirs")]
    pub openbot_dirs: Vec<String>,
}

/// Request body for `POST /api/v1/orca/bots/import/discover`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaImportDiscoverRequest {
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub roots: Option<OrcaImportRootsDto>,
    #[serde(default, alias = "exportText")]
    pub export_text: Option<String>,
    #[serde(default, alias = "importCandidate")]
    pub import_candidate: Option<BotImportCandidate>,
    #[serde(default, alias = "customName", alias = "name")]
    pub custom_name: Option<String>,
    #[serde(default, alias = "rawExportReply")]
    pub raw_export_reply: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// 7. Loopback Tool Relay Control
// ---------------------------------------------------------------------------

/// Response for `POST /api/v1/orca/relay/start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaRelayStartResponse {
    pub running: bool,
    pub addr: String,
    pub base_url: String,
}

/// Response for `GET /api/v1/orca/relay/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaRelayStatusResponse {
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub mcp_session_count: usize,
}

/// Credential injection mode for `POST /api/v1/orca/relay/tools`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum OrcaRelayToolAuthDto {
    #[default]
    None,
    StaticHeaders {
        headers: BTreeMap<String, String>,
    },
    ComposioHeaders {
        headers: BTreeMap<String, String>,
    },
    OAuth {
        credential_id: String,
    },
}

/// Optional host-side OAuth token seed passed alongside `POST /api/v1/orca/relay/tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaRelayOAuthSeedDto {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub resource: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Request body for `POST /api/v1/orca/relay/tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaRelayRegisterToolRequest {
    #[serde(alias = "credentialId")]
    pub credential_id: String,
    #[serde(alias = "toolHandle")]
    pub tool_handle: String,
    #[serde(alias = "upstreamUrl")]
    pub upstream_url: String,
    #[serde(default, alias = "allowedFolders")]
    pub allowed_folders: HashSet<String>,
    #[serde(default)]
    pub auth: OrcaRelayToolAuthDto,
    #[serde(default, alias = "oauthSeed")]
    pub oauth_seed: Option<OrcaRelayOAuthSeedDto>,
}

/// Request body for `POST /api/v1/orca/relay/grants`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaRelayRegisterGrantRequest {
    #[serde(alias = "botId")]
    pub bot_id: String,
    #[serde(default, alias = "ownerId")]
    pub owner_id: Option<String>,
    pub token: String,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default, alias = "allowedFolders")]
    pub allowed_folders: HashSet<String>,
    #[serde(default, alias = "allowedTools")]
    pub allowed_tools: HashSet<String>,
}

/// Request body for `POST /api/v1/orca/relay/revoke`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaRelayRevokeRequest {
    #[serde(default, alias = "botId")]
    pub bot_id: Option<String>,
    #[serde(default, alias = "credentialId")]
    pub credential_id: Option<String>,
}

// ---------------------------------------------------------------------------
// 8. Webhooks, Automations, Watch-PR & Telegram IM Bridge Contracts (Shared w/ M4.2)
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/orca/webhooks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaWebhookCreateRequest {
    #[serde(alias = "botId")]
    pub bot_id: String,
    #[serde(default, alias = "automationId")]
    pub automation_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub adapter: Option<WebhookAdapterId>,
    #[serde(default, alias = "rules")]
    pub filters: Option<Value>,
    #[serde(default, alias = "hourlyCap")]
    pub hourly_cap: Option<u32>,
    #[serde(default, alias = "graceMinutes")]
    pub grace_minutes: Option<u32>,
    #[serde(default, alias = "requireSignature")]
    pub require_signature: Option<bool>,
    #[serde(default, alias = "includeCopyCard")]
    pub include_copy_card: bool,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `PATCH /api/v1/orca/webhooks/{hook_id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaWebhookPatchRequest {
    #[serde(default, alias = "rules")]
    pub filters: Option<Value>,
    #[serde(default)]
    pub paused: Option<bool>,
}

/// Request body for `POST /api/v1/orca/automations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaAutomationCreateRequest {
    pub name: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(alias = "botId")]
    pub bot_id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, alias = "botTarget")]
    pub bot_target: Option<AutomationBotTarget>,
    #[serde(default, alias = "dtstartMs")]
    pub dtstart_ms: Option<i64>,
    #[serde(default)]
    pub schedule: Option<Value>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub trigger: Option<Value>,
    #[serde(default)]
    pub triggers: Vec<Value>,
    #[serde(default, alias = "runsIn")]
    pub runs_in: Option<Value>,
    #[serde(default, alias = "resultsTo")]
    pub results_to: Option<Value>,
    #[serde(default)]
    pub once: Option<OneShotWait>,
    #[serde(default, alias = "chatSessionId")]
    pub chat_session_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `PATCH /api/v1/orca/automations/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaAutomationPatchRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub triggers: Option<Vec<AutomationEventTrigger>>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/automations/watch-pr`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaWatchPrRequest {
    #[serde(alias = "botId")]
    pub bot_id: String,
    #[serde(default)]
    pub pr: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default, alias = "prNumber")]
    pub pr_number: Option<u64>,
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default, alias = "ciFinishedSupported")]
    pub ci_finished_supported: Option<bool>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// Request body for `POST /api/v1/orca/bots/{bot_id}/telegram/link`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrcaTelegramLinkRequest {
    #[serde(default, alias = "chatId")]
    pub chat_id: Option<i64>,
    #[serde(default, alias = "botUsername")]
    pub bot_username: Option<String>,
    #[serde(default, alias = "welcomeText")]
    pub welcome_text: Option<String>,
}

/// Request body for posting an interactive Orca Ask/Gate/Access card into a Telegram chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrcaTelegramCardPostRequest {
    #[serde(alias = "chatId")]
    pub chat_id: Option<i64>,
    #[serde(alias = "cardKind")]
    pub card_kind: String,
    #[serde(default, alias = "itemNumber")]
    pub item_number: Option<u64>,
    #[serde(default, alias = "gateId")]
    pub gate_id: Option<String>,
    #[serde(default, alias = "requestId")]
    pub request_id: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}
