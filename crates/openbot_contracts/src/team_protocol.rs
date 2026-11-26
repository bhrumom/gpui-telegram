//! Team API `/v1/*` routes, protocol negotiation `v1`..`v10`, complete 309-item
//! `TEAM_CAPABILITY_REGISTRY`, `v10` baseline/absent diff codecs (`OpenBot-Absent-Capabilities`),
//! bidirectional `botId <-> agentId` legacy wire walker, and WebRTC DataChannel v2 binary/JSON framing.
//! Mirrors `packages/contracts/src/team-api-routes.ts` and `team-protocol/{current,v1..v10,current-agent-keys,webrtc-framing,channels-v1,optional-routes}.ts`.

use std::collections::{BTreeMap, HashSet};

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::limits::{ContractError, non_null_option, utf16_len};

pub const TEAM_PROTOCOL_VERSION_HEADER: &str = "OpenBot-Protocol-Version";
pub const TEAM_APP_VERSION_HEADER: &str = "OpenBot-App-Version";
pub const TEAM_CAPABILITIES_HEADER: &str = "OpenBot-Capabilities";
pub const TEAM_ABSENT_CAPABILITIES_HEADER: &str = "OpenBot-Absent-Capabilities";

pub const TEAM_PROTOCOL_V1: u16 = 1;
pub const TEAM_PROTOCOL_V2: u16 = 2;
pub const TEAM_PROTOCOL_V3: u16 = 3;
pub const TEAM_PROTOCOL_V4: u16 = 4;
pub const TEAM_PROTOCOL_V5: u16 = 5;
pub const TEAM_PROTOCOL_V6: u16 = 6;
pub const TEAM_PROTOCOL_V7: u16 = 7;
pub const TEAM_PROTOCOL_V8: u16 = 8;
pub const TEAM_PROTOCOL_V9: u16 = 9;
pub const TEAM_PROTOCOL_V10: u16 = 10;

pub const TEAM_LONG_CAPABILITY_LISTS_CAPABILITY: &str = "long-capability-lists-v1";
pub const TEAM_CAPABILITY_BASELINE_CAPABILITY: &str = "capability-baseline-v1";
pub const TEAM_OPEN_PROVIDERS_CAPABILITY: &str = "open-providers-v1";
pub const TEAM_SEMANTIC_TAGS_CAPABILITY: &str = "installed-skills";

pub const TEAM_PROTOCOL_V10_CAPABILITY_LIMIT: usize = 256;
pub const TEAM_PROTOCOL_V10_ABSENT_LIMIT: usize = 256;

pub const fn capability_limit_for_protocol(version: u16) -> usize {
    match version {
        1..=6 => 64,
        7..=8 => 128,
        _ => 256,
    }
}

pub const TEAM_WS_SUBPROTOCOLS: [&str; 8] = [
    "openbot-team-v1",
    "openbot-team-v2",
    "openbot-team-v3",
    "openbot-team-v4",
    "openbot-team-v5",
    "openbot-team-v6",
    "openbot-team-v7",
    "openbot-team-v8",
];

pub const V1_HTTP_ERROR_CODES: [&str; 10] = [
    "client_update_required",
    "host_update_required",
    "protocol_error",
    "host_unavailable",
    "host_permissions_required",
    "session_capacity_reached",
    "session_expired",
    "session_revoked",
    "protocol_mismatch",
    "connection_failed",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamProtocolRange {
    pub minimum: u16,
    pub maximum: u16,
}

pub const TEAM_WEBRTC_CLIENT_PROTOCOL: TeamProtocolRange = TeamProtocolRange {
    minimum: TEAM_PROTOCOL_V3,
    maximum: TEAM_PROTOCOL_V9,
};

pub const TEAM_WEB_CLIENT_PROTOCOL: TeamProtocolRange = TeamProtocolRange {
    minimum: TEAM_PROTOCOL_V3,
    maximum: TEAM_PROTOCOL_V10,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamCapabilityEntry {
    pub capability: &'static str,
    pub since: u16,
    pub bootstrap: bool,
}

const fn cap(capability: &'static str, since: u16, bootstrap: bool) -> TeamCapabilityEntry {
    TeamCapabilityEntry {
        capability,
        since,
        bootstrap,
    }
}

/// Complete 309-item `TEAM_CAPABILITY_REGISTRY` (`team-protocol/current.ts:L560-L870`).
pub const TEAM_CAPABILITY_REGISTRY: [TeamCapabilityEntry; 309] = [
    cap("browser-secret-handoff", 1, true),
    cap("agent-runtime-snapshots", 1, true),
    cap("browser-control", 1, true),
    cap("conversation-pagination", 1, true),
    cap("direct-messages", 1, true),
    cap("hosted-site-event-markers", 1, true),
    cap("remote-desktop", 1, true),
    cap("routine-event-markers", 1, true),
    cap("routine-run-event-markers", 1, true),
    cap("sidebar-layout", 1, true),
    cap("agent-duplication", 1, true),
    cap("opencode", 1, true),
    cap("local-providers", 1, true),
    cap("local-providers-v2", 1, true),
    cap("local-providers-v3", 7, true),
    cap("open-providers-v1", 8, false),
    cap("long-capability-lists-v1", 9, true),
    cap("remote-desktop-setup", 1, true),
    cap("queue-edit-v1", 1, true),
    cap("browser-navigation", 1, true),
    cap("browser-view", 1, true),
    cap("browser-view-frame-point", 1, true),
    cap("browser-view-cursor", 1, true),
    cap("browser-view-clipboard", 1, true),
    cap("browser-view-viewport", 1, true),
    cap("browser-view-context-menu", 1, true),
    cap("agent-profile-generation", 1, true),
    cap("agent-analytics", 1, true),
    cap("host-analytics", 1, true),
    cap("installed-skills", 1, true),
    cap("agent-activity", 1, true),
    cap("conversation-unread", 1, true),
    cap("model-scoped-usage", 1, true),
    cap("agent-create-model", 1, true),
    cap("eml-attachments", 1, true),
    cap("media-attachments", 1, true),
    cap("zip-attachments", 1, false),
    cap("mp4-attachments", 7, false),
    cap("channel-chats-v1", 1, true),
    cap("channel-delete-v1", 1, true),
    cap("mcp-servers-v1", 1, true),
    cap("storage-v1", 1, true),
    cap("agent-admin-v1", 1, true),
    cap("agent-memories-v1", 7, false),
    cap("agent-session-settings-v1", 7, false),
    cap("acp-registry-v1", 7, false),
    cap("skills-admin-v1", 1, true),
    cap("skills-events-v1", 1, true),
    cap("shared-tables-v1", 1, true),
    cap("agent-install-v1", 1, true),
    cap("providers-v1", 1, true),
    cap("providers-v2", 1, true),
    cap("providers-v3", 1, true),
    cap("providers-v4", 1, true),
    cap("providers-v5", 7, false),
    cap("providers-v8", 8, false),
    cap("host-admin-v1", 1, true),
    cap("agent-update-v1", 1, true),
    cap("member-leave-v1", 1, true),
    cap("context-reset-v1", 1, true),
    cap("events-v1", 1, true),
    cap("host-update-v1", 1, true),
    cap("host-release-v1", 1, true),
    cap("quiet-turn-v1", 1, true),
    cap("host-member-update-v1", 1, true),
    cap("agent-import-v1", 1, true),
    cap("instance-transfer-v1", 7, false),
    cap("agent-publish-v1", 1, true),
    cap("live-activity-push-v1", 1, true),
    cap("hosted-sites-v1", 1, true),
    cap("message-client-id-v1", 1, true),
    cap("workspace-directory-v1", 1, true),
    cap("agent-host-settings-v1", 1, false),
    cap("mcp-sign-in-v1", 1, false),
    cap("provider-host-sign-in-v1", 7, false),
    cap("mcp-sign-in-app-v1", 7, false),
    cap("sign-in-relay-v1", 7, false),
    cap("connectors-v1", 7, false),
    cap("conversation-file-search-v1", 7, false),
    cap("routine-flows-v1", 7, false),
    cap("history-extent-v1", 7, false),
    cap("push-notifications-v1", 7, false),
    cap("push-notifications-android-v1", 7, false),
    cap("provider-accounts-v1", 7, false),
    cap("onepassword-connector-v1", 7, false),
    cap("host-timezone-v1", 7, false),
    cap("routine-recovery-v1", 7, false),
    cap("focus-use-v1", 7, false),
    cap("agent-hidden-v1", 7, false),
    cap("queue-window-v1", 7, false),
    cap("ui-blocks-v1", 7, false),
    cap("agent-tasks-v1", 7, false),
    cap("browser-user-control", 7, false),
    cap("browser-page-state", 7, false),
    cap("browser-sign-in", 7, false),
    cap("agent-memory-limit-v1", 7, false),
    cap("discord-access-v1", 7, false),
    cap("side-question-v1", 7, false),
    cap("subagents-v1", 7, false),
    cap("agent-access-v1", 7, false),
    cap("prompt-multi-select-v1", 7, false),
    cap("channel-attention-v1", 7, false),
    cap("agent-backup-models-v1", 7, false),
    cap("routine-results-v1", 7, false),
    cap("webhook-tests-v1", 7, false),
    cap("provider-usage-reset-v1", 7, false),
    cap("bitwarden-connector-v1", 7, false),
    cap("agent-task-limit-v1", 7, false),
    cap("browser-profiles-v1", 7, false),
    cap("provider-account-relay-v1", 7, false),
    cap("provider-account-relay-v2", 7, false),
    cap("storage-disk-v1", 7, false),
    cap("machine-resources-v1", 7, false),
    cap("prompt-cache-ttl-v1", 7, false),
    cap("provider-default-v1", 7, false),
    cap("claude-api-key-v1", 7, false),
    cap("workflows-v1", 7, false),
    cap("agent-threads-v1", 7, false),
    cap("agent-thread-reads-v1", 7, false),
    cap("agent-thread-files-v1", 7, false),
    cap("vault-v1", 7, false),
    cap("browser-secret-vault-v1", 7, false),
    cap("vault-import-v1", 7, false),
    cap("vault-v2", 7, false),
    cap("agent-task-ui-blocks-v1", 7, false),
    cap("context-usage-v1", 8, false),
    cap("memory-review-v1", 7, false),
    cap("agent-notification-levels-v1", 9, false),
    cap("provider-sign-out-v1", 7, false),
    cap("custom-provider-harness-v1", 7, false),
    cap("agent-new-chat-v1", 9, false),
    cap("browser-profiles-events-v1", 9, false),
    cap("agent-plan-card-v1", 9, false),
    cap("ui-block-auto-answer-v1", 9, false),
    cap("agent-folders-v1", 9, false),
    cap("host-key-rotation-v1", 9, false),
    cap("agent-thread-branches-v1", 9, false),
    cap("custom-agents-v1", 9, false),
    cap("channel-task-cancel-v1", 9, false),
    cap("workflows-github-v1", 9, false),
    cap("svg-attachments", 9, false),
    cap("agent-hidden-work", 9, false),
    cap("mcp-accounts-v1", 9, false),
    cap("sidebar-pins-v1", 9, false),
    cap("sidebar-folders-v1", 9, false),
    cap("reasoning-efforts-v1", 9, false),
    cap("server-audit-v1", 9, false),
    cap("agent-fast-mode-v1", 9, false),
    cap("context-compact-v1", 9, false),
    cap("agent-instructions-v1", 9, false),
    cap("ui-block-vault-v1", 9, false),
    cap("local-servers-v1", 9, false),
    cap("computer-activity-v1", 9, false),
    cap("model-visibility-v1", 9, false),
    cap("agent-thread-replies-v1", 9, false),
    cap("agent-colors-v1", 9, false),
    cap("chat-colors-v1", 9, false),
    cap("webhook-sender-secret-v1", 9, false),
    cap("whatsapp-connector-v1", 9, false),
    cap("message-delete-v1", 9, false),
    cap("conversation-unread-from-v1", 9, false),
    cap("workflows-chats-v1", 9, false),
    cap("password-managers-v1", 9, false),
    cap("browser-proxies-v1", 9, false),
    cap("browser-egress-v1", 9, false),
    cap("codex-api-key-v1", 9, false),
    cap("channel-isolation-v1", 9, false),
    cap("agent-thread-pages-v1", 9, false),
    cap("any-file-attachments", 9, false),
    cap("routine-run-place-v1", 9, false),
    cap("host-personality-v1", 9, false),
    cap("mcp-member-access-v1", 9, false),
    cap("mcp-access-v1", 9, false),
    cap("live-voice-v1", 9, false),
    cap("agent-welcome-v1", 9, false),
    cap("skill-learning-v1", 9, false),
    cap("custom-provider-edit-v1", 9, false),
    cap("mcp-many-servers-v1", 9, false),
    cap("vault-v3", 9, false),
    cap("instance-transfer-chunks-v1", 9, false),
    cap("chat-history-v1", 9, false),
    cap("browsing-memory-v1", 9, false),
    cap("browser-teach-v1", 9, false),
    cap("channel-picture-v1", 9, false),
    cap("agent-import-v2", 9, false),
    cap("thread-message-search-v1", 9, false),
    cap("workflows-channels-v1", 9, false),
    cap("agent-waits-v1", 9, false),
    cap("message-pins-v1", 9, false),
    cap("agent-queue-task-v1", 9, false),
    cap("background-processes-v1", 9, false),
    cap("provider-use-v1", 9, false),
    cap("routines-once-v1", 9, false),
    cap("routine-run-command-v1", 9, false),
    cap("claude-user-setup-v1", 9, false),
    cap("agent-thread-queue-v1", 9, false),
    cap("agent-usage-limit-v1", 9, false),
    cap("hermes-import-v1", 9, false),
    cap("provider-detection-v1", 9, false),
    cap("push-notification-threads-v1", 9, false),
    cap("claude-compact-window-v1", 9, false),
    cap("provider-account-models-v1", 9, false),
    cap("channel-browser-v1", 9, false),
    cap("storage-channel-v1", 9, false),
    cap("whatsapp-working-message-v1", 9, false),
    cap("library-folders-v1", 9, false),
    cap("library-folders-events-v1", 9, false),
    cap("provider-account-pool-v1", 9, false),
    cap("video-generation-v1", 9, false),
    cap("channel-seen-subtasks-v1", 9, false),
    cap("mistral-code-sign-in-v1", 9, false),
    cap("instance-transfer-full-copy-v1", 9, false),
    cap("github-skills-v1", 9, false),
    cap("host-restart-v1", 9, false),
    cap("vault-v4", 9, false),
    cap("workflows-media-v1", 9, false),
    cap("agent-defaults-v1", 9, false),
    cap("agent-speed-v1", 9, false),
    cap("workflows-sunra-v1", 9, false),
    cap("workflows-media-account-v1", 9, false),
    cap("usage-tertiary-v1", 9, false),
    cap("host-turn-limits-v1", 9, false),
    cap("storage-user-files-v1", 9, false),
    cap("workflows-media-transparent-v1", 9, false),
    cap("pending-approval-v1", 9, false),
    cap("provider-account-keys-v1", 9, false),
    cap("usage-credits-v1", 9, false),
    cap("channel-task-waits-v1", 9, false),
    cap("routine-flow-stop-v1", 9, false),
    cap("agent-pending-runtime-v1", 9, false),
    cap("browser-sign-in-storage-v1", 9, false),
    cap("host-appearance-v1", 9, false),
    cap("sign-in-relay-deep-link-v1", 9, false),
    cap("provider-account-code-v1", 9, false),
    cap("browser-vault-skip-v1", 9, false),
    cap("model-steering-v1", 9, false),
    cap("memory-changes-v1", 9, false),
    cap("phone-feedback-v1", 9, false),
    cap("workflows-higgsfield-v1", 9, false),
    cap("provider-environment-v1", 9, false),
    cap("custom-provider-harness-choice-v1", 9, false),
    cap("feedback-withdraw-v1", 9, false),
    cap("whatsapp-status-reactions-v1", 9, false),
    cap("crm-v1", 9, false),
    cap("setting-link-markers-v1", 9, false),
    cap("workspace-only-process-limit-v1", 9, false),
    cap("mac-apps-limit-v1", 9, false),
    cap("provider-model-source-v1", 9, false),
    cap("workflows-media-size-v1", 9, false),
    cap("routine-pause-reasons-v1", 9, false),
    cap("agent-thread-requests-v1", 9, false),
    cap("browser-view-page-copy-v1", 9, false),
    cap("ui-block-preview-edit-v1", 9, false),
    cap("workflows-media-catalog-v1", 9, false),
    cap("agent-active-queues-v1", 9, false),
    cap("agent-create-operation-v1", 9, false),
    cap("capability-baseline-v1", 10, false),
    cap("server-safe-mode-v1", 10, false),
    cap("webhook-agent-secret-v1", 10, false),
    cap("agent-background-work-v1", 10, false),
    cap("attachment-preview-v1", 10, false),
    cap("workflows-media-service-account-v1", 10, false),
    cap("routine-quiet-answers-v1", 10, false),
    cap("skill-problems-v1", 10, false),
    cap("routine-results-whatsapp-v1", 10, false),
    cap("queue-hold-v1", 10, false),
    cap("owner-transfer-v1", 10, false),
    cap("vault-member-save-v1", 10, false),
    cap("workflows-agent-files-v1", 10, false),
    cap("workflows-media-catalog-v2", 10, false),
    cap("workflows-step-settings-v1", 10, false),
    cap("usage-groups-v1", 10, false),
    cap("queue-hold-sign-in-v1", 10, false),
    cap("provider-sign-in-relay-v2", 10, false),
    cap("pending-prompts-v1", 10, false),
    cap("live-voice-account-v1", 10, false),
    cap("side-question-threads-v1", 10, false),
    cap("workflow-folders-v1", 10, false),
    cap("agent-builtin-memory-v1", 10, false),
    cap("workflows-answers-v1", 10, false),
    cap("storage-workflow-files-v1", 10, false),
    cap("provider-many-accounts-v1", 10, false),
    cap("channel-folders-v1", 10, false),
    cap("workflows-credit-media-v1", 10, false),
    cap("prompt-cache-ttl-pi-v1", 10, false),
    cap("cache-expiry-compaction-v1", 10, false),
    cap("agent-archived-threads-v1", 10, false),
    cap("host-voice-model-v1", 10, false),
    cap("usage-read-failed-v1", 10, false),
    cap("workflows-pause-v1", 10, false),
    cap("workflows-portable-v1", 10, false),
    cap("model-visibility-kept-v1", 10, false),
    cap("ui-block-location-v1", 10, false),
    cap("host-signal-home-v1", 10, false),
    cap("workflows-credit-media-unavailable-v1", 10, false),
    cap("antigravity-code-sign-in-v1", 10, false),
    cap("whatsapp-read-only-v1", 10, false),
    cap("voice-transcription-v1", 10, false),
    cap("client-browser-v1", 10, false),
    cap("channel-page-tasks-v1", 10, false),
    cap("push-notification-workflows-v1", 10, false),
    cap("prompt-detail-v1", 10, false),
    cap("host-turn-limits-events-v1", 10, false),
    cap("media-approval-credits-v1", 10, false),
    cap("media-approval-choice-v1", 10, false),
    cap("openbot-credits-events-v1", 10, false),
    cap("browser-view-frame-size", 10, false),
    cap("agent-error-link-v1", 10, false),
    cap("browser-permissions", 10, false),
];

pub fn team_current_capabilities() -> Vec<&'static str> {
    TEAM_CAPABILITY_REGISTRY.iter().map(|e| e.capability).collect()
}

pub fn team_bootstrap_capabilities() -> Vec<&'static str> {
    TEAM_CAPABILITY_REGISTRY
        .iter()
        .filter(|e| e.bootstrap)
        .map(|e| e.capability)
        .collect()
}

pub fn team_protocol_v10_baseline_capabilities() -> Vec<&'static str> {
    TEAM_CAPABILITY_REGISTRY
        .iter()
        .filter(|e| e.since <= 9)
        .map(|e| e.capability)
        .collect()
}

pub fn capability_since(capability: &str) -> u16 {
    TEAM_CAPABILITY_REGISTRY
        .iter()
        .find(|e| e.capability == capability)
        .map_or(1, |e| e.since)
}

pub fn team_capabilities_for_protocol(protocol: Option<u16>) -> Vec<&'static str> {
    let Some(v) = protocol else {
        return team_bootstrap_capabilities();
    };
    if v < 7 {
        team_bootstrap_capabilities()
    } else if v >= 10 {
        team_current_capabilities()
    } else if v == 9 {
        team_protocol_v10_baseline_capabilities()
    } else {
        let max_since = if v == 8 { 8 } else { 7 };
        TEAM_CAPABILITY_REGISTRY
            .iter()
            .filter(|e| e.since <= max_since)
            .map(|e| e.capability)
            .collect()
    }
}

pub fn team_capabilities_visible_to_protocol<'a>(
    capabilities: &[&'a str],
    protocol: u16,
) -> Vec<&'a str> {
    let newest = if protocol >= 10 {
        10
    } else if protocol >= 9 {
        9
    } else {
        8
    };
    capabilities
        .iter()
        .copied()
        .filter(|c| capability_since(c) <= newest)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamCapabilitiesV10 {
    pub capabilities: Vec<String>,
    pub absent: Vec<String>,
}

/// Encodes `v10` delta + absent lists (`team-protocol/current.ts:encodeTeamCapabilitiesV10`, `L987-L996`).
pub fn encode_team_capabilities_v10(capabilities: &[String]) -> TeamCapabilitiesV10 {
    let baseline = team_protocol_v10_baseline_capabilities();
    let baseline_set: HashSet<&str> = baseline.iter().copied().collect();
    let mut served_unique = Vec::new();
    let mut served_set = HashSet::new();
    for cap_str in capabilities {
        if served_set.insert(cap_str.as_str()) {
            served_unique.push(cap_str.clone());
        }
    }
    let delta = served_unique
        .into_iter()
        .filter(|c| {
            !baseline_set.contains(c.as_str()) || c == TEAM_LONG_CAPABILITY_LISTS_CAPABILITY
        })
        .collect();
    let absent = baseline
        .into_iter()
        .filter(|c| !served_set.contains(*c))
        .map(str::to_string)
        .collect();
    TeamCapabilitiesV10 {
        capabilities: delta,
        absent,
    }
}

/// Expands `v10` delta + absent lists back into the full capability list (`current.ts:expandTeamCapabilitiesV10`).
pub fn expand_team_capabilities_v10(lists: &TeamCapabilitiesV10) -> Vec<String> {
    let absent: HashSet<&str> = lists.absent.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for base in team_protocol_v10_baseline_capabilities() {
        if !absent.contains(base) && seen.insert(base.to_string()) {
            out.push(base.to_string());
        }
    }
    for extra in &lists.capabilities {
        if seen.insert(extra.clone()) {
            out.push(extra.clone());
        }
    }
    out
}

pub fn team_capability_headers(
    protocol: Option<u16>,
    capabilities: &[String],
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if protocol.is_none_or(|p| p < TEAM_PROTOCOL_V10) {
        headers.insert(TEAM_CAPABILITIES_HEADER.to_string(), capabilities.join(","));
        return headers;
    }
    let lists = encode_team_capabilities_v10(capabilities);
    headers.insert(
        TEAM_CAPABILITIES_HEADER.to_string(),
        lists.capabilities.join(","),
    );
    if !lists.absent.is_empty() {
        headers.insert(
            TEAM_ABSENT_CAPABILITIES_HEADER.to_string(),
            lists.absent.join(","),
        );
    }
    headers
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamProtocolUpdateDirection {
    ClientUpdateRequired,
    HostUpdateRequired,
}

pub fn highest_common_team_protocol(
    local: TeamProtocolRange,
    remote: TeamProtocolRange,
) -> Option<u16> {
    let minimum = local.minimum.max(remote.minimum);
    let maximum = local.maximum.min(remote.maximum);
    (minimum <= maximum).then_some(maximum)
}

pub fn team_protocol_update_direction(
    local: TeamProtocolRange,
    remote: TeamProtocolRange,
) -> Option<TeamProtocolUpdateDirection> {
    if highest_common_team_protocol(local, remote).is_some() {
        return None;
    }
    if local.maximum < remote.minimum {
        Some(TeamProtocolUpdateDirection::ClientUpdateRequired)
    } else {
        Some(TeamProtocolUpdateDirection::HostUpdateRequired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamProtocolSupport {
    pub app_version: String,
    pub protocol: TeamProtocolRange,
    pub capabilities: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub absent: Option<Vec<String>>,
}

pub fn is_capability_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn decode_team_protocol_support_expanded(
    support: &TeamProtocolSupport,
) -> Result<TeamProtocolSupport, ContractError> {
    if utf16_len(&support.app_version) > 64
        || support.protocol.minimum == 0
        || support.protocol.minimum > support.protocol.maximum
        || support.capabilities.len() > TEAM_PROTOCOL_V10_CAPABILITY_LIMIT
        || !support.capabilities.iter().all(|c| is_capability_slug(c))
    {
        return Err(ContractError::TeamProtocol(
            "Invalid Team API compatibility response.".to_string(),
        ));
    }
    if let Some(absent) = &support.absent {
        if absent.len() > TEAM_PROTOCOL_V10_ABSENT_LIMIT
            || !absent.iter().all(|c| is_capability_slug(c))
        {
            return Err(ContractError::TeamProtocol(
                "Invalid Team API v10 absent capability list.".to_string(),
            ));
        }
        let expanded = expand_team_capabilities_v10(&TeamCapabilitiesV10 {
            capabilities: support.capabilities.clone(),
            absent: absent.clone(),
        });
        return Ok(TeamProtocolSupport {
            app_version: support.app_version.clone(),
            protocol: support.protocol,
            capabilities: expanded,
            absent: None,
        });
    }
    let mut seen = HashSet::new();
    let deduped = support
        .capabilities
        .iter()
        .filter(|c| seen.insert(c.as_str()))
        .cloned()
        .collect();
    Ok(TeamProtocolSupport {
        app_version: support.app_version.clone(),
        protocol: support.protocol,
        capabilities: deduped,
        absent: None,
    })
}

fn translate_agent_key(entry_key: &str, parent_key: &str, to_wire: bool) -> String {
    if parent_key == "marketplaceSource" {
        return match (to_wire, entry_key) {
            (true, "listingId") => "agentId".to_string(),
            (false, "agentId") => "listingId".to_string(),
            _ => entry_key.to_string(),
        };
    }
    if to_wire {
        match entry_key {
            "agent" => "bot",
            "agentId" => "botId",
            "agents" => "bots",
            "ownerAgentId" => "ownerBotId",
            "recipientAgentId" => "recipientBotId",
            "recipientAgentIds" => "recipientBotIds",
            "senderAgentId" => "senderBotId",
            "typingAgentId" => "typingBotId",
            other => other,
        }
        .to_string()
    } else {
        match entry_key {
            "bot" => "agent",
            "botId" => "agentId",
            "bots" => "agents",
            "ownerBotId" => "ownerAgentId",
            "recipientBotId" => "recipientAgentId",
            "recipientBotIds" => "recipientAgentIds",
            "senderBotId" => "senderAgentId",
            "typingBotId" => "typingAgentId",
            other => other,
        }
        .to_string()
    }
}

fn translate_agent_value(raw: &str, parent_key: &str, to_wire: bool) -> String {
    match (to_wire, parent_key, raw) {
        (true, "kind" | "origin", "agent") => "bot".to_string(),
        (false, "kind" | "origin", "bot") => "agent".to_string(),
        (true, "type", "agents-changed") => "bots-changed".to_string(),
        (false, "type", "bots-changed") => "agents-changed".to_string(),
        _ => raw.to_string(),
    }
}

fn walk_agent_keys(value: &serde_json::Value, to_wire: bool, key: &str) -> serde_json::Value {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            value.clone()
        }
        serde_json::Value::String(s) => {
            serde_json::Value::String(translate_agent_value(s, key, to_wire))
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter().map(|v| walk_agent_keys(v, to_wire, key)).collect(),
        ),
        serde_json::Value::Object(obj) => {
            let dynamic_map = matches!(key, "responses" | "references" | "answers");
            let mut mapped = serde_json::Map::with_capacity(obj.len());
            for (k, v) in obj {
                if dynamic_map {
                    mapped.insert(k.clone(), walk_agent_keys(v, to_wire, ""));
                } else {
                    let next_key = translate_agent_key(k, key, to_wire);
                    let next_val = if k == "layout" {
                        v.clone()
                    } else {
                        walk_agent_keys(v, to_wire, k)
                    };
                    mapped.insert(next_key, next_val);
                }
            }
            serde_json::Value::Object(mapped)
        }
    }
}

pub fn is_untranslated_agent_keys_path(path: &str) -> bool {
    let pathname = Url::parse("http://openbot.invalid")
        .and_then(|base| base.join(path))
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| path.to_string());
    pathname.starts_with("/v1/sidebar-layout") || pathname == "/v1/agents/conversation-reads"
}

/// Converts current `agentId`/`agent` keys to legacy wire `botId`/`bot` (`current-agent-keys.ts:L80`).
pub fn to_wire_agent_keys(value: &serde_json::Value) -> serde_json::Value {
    walk_agent_keys(value, true, "")
}

/// Converts legacy wire `botId`/`bot` keys to current `agentId`/`agent` (`current-agent-keys.ts:L84`).
pub fn to_current_agent_keys(value: &serde_json::Value) -> serde_json::Value {
    walk_agent_keys(value, false, "")
}

pub fn to_wire_agent_keys_for_path(path: &str, value: &serde_json::Value) -> serde_json::Value {
    if is_untranslated_agent_keys_path(path) {
        value.clone()
    } else {
        to_wire_agent_keys(value)
    }
}

pub fn to_current_agent_keys_for_path(path: &str, value: &serde_json::Value) -> serde_json::Value {
    if is_untranslated_agent_keys_path(path) {
        value.clone()
    } else {
        to_current_agent_keys(value)
    }
}

pub const TEAM_PROTOCOL_V2_CHANNEL_RPC: &str = "openbot-team-v2-rpc";
pub const TEAM_PROTOCOL_V2_CHANNEL_EVENTS: &str = "openbot-team-v2-events";
pub const TEAM_PROTOCOL_V2_CHANNEL_FILES: &str = "openbot-team-v2-files";
pub const TEAM_PROTOCOL_V2_MAX_FILE_BYTES: usize = 100 * 1024 * 1024;
pub const TEAM_PROTOCOL_V2_MAX_FILE_SET_BYTES: usize = 250 * 1024 * 1024;
pub const TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES: usize = 64 * 1024;
pub const TEAM_PROTOCOL_V2_MAX_JSON_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const TEAM_PROTOCOL_V2_BUFFERED_AMOUNT_HIGH_WATER: usize = 4 * 1024 * 1024;
pub const TEAM_PROTOCOL_V2_BUFFERED_AMOUNT_LOW_WATER: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamProtocolV2FileChunk {
    pub transfer_id: String,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

fn is_v2_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-'))
}

pub const TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET: u64 = (1u64 << 53) - 1;

/// Binary file chunk encoder `[2, id_len, id_utf8, u64_be_offset, bytes]` (`webrtc-framing.ts:L286-L303`).
pub fn encode_team_protocol_v2_file_chunk(
    chunk: &TeamProtocolV2FileChunk,
    max_message_size: Option<usize>,
) -> Result<Vec<u8>, ContractError> {
    if !is_v2_identifier(&chunk.transfer_id)
        || chunk.offset > TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET
        || chunk.bytes.is_empty()
    {
        return Err(ContractError::TeamProtocol("Invalid Team protocol v2 file chunk.".to_string()));
    }
    let id_bytes = chunk.transfer_id.as_bytes();
    let frame_size = 10 + id_bytes.len() + chunk.bytes.len();
    let limit = max_message_size
        .map_or(TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES, |m| {
            m.min(TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES)
        });
    if frame_size > limit {
        return Err(ContractError::TeamProtocol(
            "Invalid Team protocol v2 file chunk size.".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(frame_size);
    out.push(TEAM_PROTOCOL_V2 as u8);
    out.push(id_bytes.len() as u8);
    out.extend_from_slice(id_bytes);
    out.extend_from_slice(&chunk.offset.to_be_bytes());
    out.extend_from_slice(&chunk.bytes);
    Ok(out)
}

/// Binary file chunk decoder (`webrtc-framing.ts:L305-L323`).
pub fn decode_team_protocol_v2_file_chunk(
    frame: &[u8],
) -> Result<TeamProtocolV2FileChunk, ContractError> {
    if frame.len() > TEAM_PROTOCOL_V2_MAX_BINARY_FRAME_BYTES
        || frame.len() < 12
        || frame[0] != TEAM_PROTOCOL_V2 as u8
    {
        return Err(ContractError::TeamProtocol("Invalid Team protocol v2 file chunk.".to_string()));
    }
    let id_len = usize::from(frame[1]);
    let payload_offset = 10 + id_len;
    if id_len == 0 || payload_offset >= frame.len() {
        return Err(ContractError::TeamProtocol("Invalid Team protocol v2 file chunk.".to_string()));
    }
    let transfer_id = std::str::from_utf8(&frame[2..2 + id_len])
        .map_err(|_| ContractError::TeamProtocol("Invalid UTF-8 transferId.".to_string()))?
        .to_string();
    if !is_v2_identifier(&transfer_id) {
        return Err(ContractError::TeamProtocol("Invalid transferId.".to_string()));
    }
    let offset_bytes: [u8; 8] = frame[2 + id_len..10 + id_len]
        .try_into()
        .expect("8-byte offset");
    let offset = u64::from_be_bytes(offset_bytes);
    if offset > TEAM_PROTOCOL_V2_MAX_SAFE_OFFSET {
        return Err(ContractError::TeamProtocol("Invalid Team protocol v2 file chunk.".to_string()));
    }
    Ok(TeamProtocolV2FileChunk {
        transfer_id,
        offset,
        bytes: frame[payload_offset..].to_vec(),
    })
}

fn is_v2_operation(s: &str) -> bool {
    let bytes = s.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..].iter().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn is_v2_json(value: &serde_json::Value, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => true,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        serde_json::Value::Array(arr) => {
            arr.len() <= 10_000 && arr.iter().all(|item| is_v2_json(item, depth + 1))
        }
        serde_json::Value::Object(map) => {
            map.len() <= 10_000 && map.values().all(|item| is_v2_json(item, depth + 1))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamProtocolV2Error {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub status: Option<u16>,
}

impl TeamProtocolV2Error {
    pub fn is_valid(&self) -> bool {
        is_v2_operation(&self.code)
            && !self.message.is_empty()
            && utf16_len(&self.message) <= 512
            && self.status.is_none_or(|s| (400..=599).contains(&s))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TeamProtocolV2RpcFrame {
    Request {
        version: u8,
        request_id: String,
        operation: String,
        payload: serde_json::Value,
    },
    Response {
        version: u8,
        request_id: String,
        result: Option<serde_json::Value>,
        error: Option<TeamProtocolV2Error>,
    },
}

impl TeamProtocolV2RpcFrame {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Request {
                version,
                request_id,
                operation,
                payload,
            } => {
                *version == TEAM_PROTOCOL_V2 as u8
                    && is_v2_identifier(request_id)
                    && is_v2_operation(operation)
                    && is_v2_json(payload, 0)
            }
            Self::Response {
                version,
                request_id,
                result,
                error,
            } => {
                *version == TEAM_PROTOCOL_V2 as u8
                    && is_v2_identifier(request_id)
                    && (result.is_some() != error.is_some())
                    && result.as_ref().is_none_or(|r| is_v2_json(r, 0))
                    && error.as_ref().is_none_or(TeamProtocolV2Error::is_valid)
            }
        }
    }
}

impl Serialize for TeamProtocolV2RpcFrame {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Request {
                version,
                request_id,
                operation,
                payload,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("type", "request")?;
                map.serialize_entry("version", version)?;
                map.serialize_entry("requestId", request_id)?;
                map.serialize_entry("operation", operation)?;
                map.serialize_entry("payload", payload)?;
                map.end()
            }
            Self::Response {
                version,
                request_id,
                result,
                error,
            } => {
                let extra = usize::from(result.is_some()) + usize::from(error.is_some());
                let mut map = serializer.serialize_map(Some(3 + extra))?;
                map.serialize_entry("type", "response")?;
                map.serialize_entry("version", version)?;
                map.serialize_entry("requestId", request_id)?;
                if let Some(res) = result {
                    map.serialize_entry("result", res)?;
                }
                if let Some(err) = error {
                    map.serialize_entry("error", err)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for TeamProtocolV2RpcFrame {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = val.as_object() else {
            return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
        };
        let Some(version_u64) = obj.get("version").and_then(serde_json::Value::as_u64) else {
            return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
        };
        if version_u64 != u64::from(TEAM_PROTOCOL_V2) {
            return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
        }
        let version = version_u64 as u8;
        let Some(request_id) = obj.get("requestId").and_then(serde_json::Value::as_str) else {
            return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
        };
        if !is_v2_identifier(request_id) {
            return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
        }
        match obj.get("type").and_then(serde_json::Value::as_str) {
            Some("request") => {
                let Some(operation) = obj.get("operation").and_then(serde_json::Value::as_str)
                else {
                    return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
                };
                let Some(payload) = obj.get("payload") else {
                    return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
                };
                if !is_v2_operation(operation) || !is_v2_json(payload, 0) {
                    return Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame."));
                }
                Ok(Self::Request {
                    version,
                    request_id: request_id.to_string(),
                    operation: operation.to_string(),
                    payload: payload.clone(),
                })
            }
            Some("response") => {
                let has_result = obj.contains_key("result");
                let has_error = obj.contains_key("error");
                if has_result == has_error {
                    return Err(serde::de::Error::custom(
                        "Invalid Team protocol v2 RPC response.",
                    ));
                }
                if has_result {
                    let res = &obj["result"];
                    if !is_v2_json(res, 0) {
                        return Err(serde::de::Error::custom(
                            "Invalid Team protocol v2 RPC response.",
                        ));
                    }
                    return Ok(Self::Response {
                        version,
                        request_id: request_id.to_string(),
                        result: Some(res.clone()),
                        error: None,
                    });
                }
                let err: TeamProtocolV2Error =
                    serde_json::from_value(obj["error"].clone()).map_err(serde::de::Error::custom)?;
                if !err.is_valid() {
                    return Err(serde::de::Error::custom(
                        "Invalid Team protocol v2 RPC response.",
                    ));
                }
                Ok(Self::Response {
                    version,
                    request_id: request_id.to_string(),
                    result: None,
                    error: Some(err),
                })
            }
            _ => Err(serde::de::Error::custom("Invalid Team protocol v2 RPC frame.")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TeamProtocolV2AuthFrame {
    AuthInit {
        version: u8,
        ticket: String,
        #[serde(rename = "clientPublicKey")]
        client_public_key: String,
        #[serde(rename = "clientNonce")]
        client_nonce: String,
        signature: String,
    },
    AuthReady {
        version: u8,
        #[serde(rename = "clientNonce")]
        client_nonce: String,
        #[serde(rename = "hostNonce")]
        host_nonce: String,
        signature: String,
    },
    AuthComplete {
        version: u8,
        #[serde(rename = "clientNonce")]
        client_nonce: String,
        #[serde(rename = "hostNonce")]
        host_nonce: String,
    },
    AuthConfirmed {
        version: u8,
        #[serde(rename = "clientNonce")]
        client_nonce: String,
        #[serde(rename = "hostNonce")]
        host_nonce: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TeamProtocolV2EventFrame {
    Event {
        version: u8,
        sequence: u64,
        payload: serde_json::Value,
    },
    EventAck {
        version: u8,
        #[serde(rename = "throughSequence")]
        through_sequence: u64,
    },
    EventReset {
        version: u8,
        #[serde(rename = "nextSequence")]
        next_sequence: u64,
    },
    EventControl {
        version: u8,
        control: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TeamProtocolV2FileControlFrame {
    FileOpen {
        version: u8,
        #[serde(rename = "transferId")]
        transfer_id: String,
        name: String,
        size: u64,
        #[serde(rename = "mimeType")]
        mime_type: String,
        sha256: String,
    },
    FileAck {
        version: u8,
        #[serde(rename = "transferId")]
        transfer_id: String,
        #[serde(rename = "receivedThrough")]
        received_through: u64,
    },
    FileComplete {
        version: u8,
        #[serde(rename = "transferId")]
        transfer_id: String,
    },
    FileCancel {
        version: u8,
        #[serde(rename = "transferId")]
        transfer_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamProtocolV2AuthTranscriptInput<'a> {
    pub host_id: &'a str,
    pub session_id: &'a str,
    pub ticket: &'a str,
    pub client_public_key: &'a str,
    pub client_nonce: &'a str,
    pub host_nonce: Option<&'a str>,
    pub client_fingerprint: &'a str,
    pub host_fingerprint: &'a str,
}

pub fn team_protocol_v2_authentication_transcript(
    input: &TeamProtocolV2AuthTranscriptInput<'_>,
) -> String {
    serde_json::to_string(&[
        "openbot-team-v2-auth",
        input.host_id,
        input.session_id,
        input.ticket,
        input.client_public_key,
        input.client_nonce,
        input.host_nonce.unwrap_or(""),
        input.client_fingerprint,
        input.host_fingerprint,
    ])
    .expect("string array serializes")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamApiRoutes {
    pub health: &'static str,
    pub capabilities: &'static str,
    pub analytics: &'static str,
    pub compatibility: &'static str,
    pub identity: &'static str,
    pub events: &'static str,
    pub me: &'static str,
    pub attachments: &'static str,
    pub shared_files: &'static str,
    pub workspace_files: &'static str,
    pub remote_desktop_upgrade: &'static str,
    pub join_server: &'static str,
    pub join_account: &'static str,
    pub invitation_preview: &'static str,
    pub auth_login: &'static str,
    pub auth_account: &'static str,
    pub auth_logout: &'static str,
    pub auth_password: &'static str,
    pub team_presence: &'static str,
    pub team_logo: &'static str,
    pub team_members: &'static str,
    pub team_leave: &'static str,
    pub team_invites: &'static str,
    pub team_sessions: &'static str,
    pub direct_threads: &'static str,
    pub direct_messages: &'static str,
    pub messages_search: &'static str,
    pub browser_open: &'static str,
    pub browser_tabs: &'static str,
    pub browser_control: &'static str,
    pub sidebar_layout: &'static str,
    pub sidebar_layout_actions: &'static str,
    pub prompts_respond: &'static str,
    pub approvals_respond: &'static str,
    pub browser_takeovers_respond: &'static str,
    pub agents_generate_profile: &'static str,
    pub agents_save_profile: &'static str,
    pub agents_all: &'static str,
    pub agents_status: &'static str,
    pub agents_usage: &'static str,
    pub agents_models: &'static str,
    pub agents_conversation_reads: &'static str,
    pub channels_list: &'static str,
    pub channels_read: &'static str,
    pub channels_command: &'static str,
    pub channels_delete: &'static str,
    pub channels_memories: &'static str,
    pub channels_routines: &'static str,
    pub workflows_list: &'static str,
    pub crm_overview: &'static str,
    pub crm_apply: &'static str,
    pub vault_list: &'static str,
}

pub const TEAM_API_ROUTES: TeamApiRoutes = TeamApiRoutes {
    health: "/v1/health",
    capabilities: "/v1/capabilities",
    analytics: "/v1/analytics",
    compatibility: "/v1/compatibility",
    identity: "/v1/identity",
    events: "/v1/events",
    me: "/v1/me",
    attachments: "/v1/attachments",
    shared_files: "/v1/shared-files",
    workspace_files: "/v1/workspace-files",
    remote_desktop_upgrade: "/v1/remote-desktop",
    join_server: "/v1/join",
    join_account: "/v1/join/account",
    invitation_preview: "/v1/invitations/preview",
    auth_login: "/v1/auth/login",
    auth_account: "/v1/auth/account",
    auth_logout: "/v1/auth/logout",
    auth_password: "/v1/auth/password",
    team_presence: "/v1/team/presence",
    team_logo: "/v1/team/logo",
    team_members: "/v1/team/members",
    team_leave: "/v1/team/leave",
    team_invites: "/v1/team/invites",
    team_sessions: "/v1/team/sessions",
    direct_threads: "/v1/direct/threads",
    direct_messages: "/v1/direct/messages",
    messages_search: "/v1/messages/search",
    browser_open: "/v1/browser/open",
    browser_tabs: "/v1/browser/tabs",
    browser_control: "/v1/browser/control",
    sidebar_layout: "/v1/sidebar-layout",
    sidebar_layout_actions: "/v1/sidebar-layout/actions",
    prompts_respond: "/v1/prompts/respond",
    approvals_respond: "/v1/approvals/respond",
    browser_takeovers_respond: "/v1/browser-takeovers/respond",
    agents_generate_profile: "/v1/agents/profile/generate",
    agents_save_profile: "/v1/agents/profile/save",
    agents_all: "/v1/agents",
    agents_status: "/v1/agents/status",
    agents_usage: "/v1/agents/usage",
    agents_models: "/v1/agents/models",
    agents_conversation_reads: "/v1/agents/conversation-reads",
    channels_list: "/v1/channels",
    channels_read: "/v1/channels/read",
    channels_command: "/v1/channels/commands",
    channels_delete: "/v1/channels/delete",
    channels_memories: "/v1/channels/memories",
    channels_routines: "/v1/channels/routines",
    workflows_list: "/v1/workflows",
    crm_overview: "/v1/crm/overview",
    crm_apply: "/v1/crm/apply",
    vault_list: "/v1/vault",
};

fn encode_segment(segment: &str) -> String {
    crate::tags::encode_chat_tag_component(segment)
}

pub fn team_agent_route(agent_id: &str) -> String {
    format!("/v1/agents/{}", encode_segment(agent_id))
}

pub fn team_agent_duplicate_route(agent_id: &str) -> String {
    format!("/v1/agents/{}/duplicate", encode_segment(agent_id))
}

pub fn team_agent_messages_route(agent_id: &str) -> String {
    format!("/v1/agents/{}/messages", encode_segment(agent_id))
}

pub fn team_agent_queue_edit_route(agent_id: &str) -> String {
    format!("/v1/agents/{}/queue/edit", encode_segment(agent_id))
}
