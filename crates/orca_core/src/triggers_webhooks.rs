//! Pure-Rust Automation Event Triggers (31 kinds across 6 sources), Bot Targets,
//! 6-Adapter Webhook Contract, JSON-Path Filters, 14-State Call Lifecycle,
//! Rolling-Hour Cap Tracker, Zero-Leak `WebhookTriggerEvent`, `OneShotWait`, and
//! `WatchPrPolicy` (Deliverable #8).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BotError, BotErrorCode, WEBHOOK_ACCOUNT_HOURLY_CAP, WEBHOOK_HOOK_HOURLY_CAP};

// ---------------------------------------------------------------------------
// Canonical Constants (Exact Upstream Values)
// ---------------------------------------------------------------------------

pub const MAX_AUTOMATION_EVENT_TRIGGERS: usize = 20;
pub const MAX_TRIGGER_PLACES: usize = 10;
pub const MAX_TRIGGER_ID_LENGTH: usize = 128;
pub const MAX_TRIGGER_FILTER_LENGTH: usize = 200;
pub const MAX_RUN_EVENT_KEYS: usize = 50;
pub const MAX_AUTOMATION_EVENT_TEXT_LENGTH: usize = 4_096;

pub const MAX_GOOGLE_FILTER_LENGTH: usize = 200;
pub const MAX_GOOGLE_ADDRESSES: usize = 10;
pub const CALENDAR_MAX_MINUTES_BEFORE: u32 = 24 * 60;

pub const EVENT_ONLY_SCHEDULE_DATE: &str = "9999-01-01";
pub const EVENT_ONLY_SCHEDULE_RRULE: &str = "FREQ=DAILY;BYHOUR=0;BYMINUTE=0";
pub const EVENT_ONLY_SCHEDULE_DTSTART_MS: i64 = 253_370_764_800_000;

pub const DEFAULT_AUTOMATION_PRECHECK_TIMEOUT_SECONDS: u32 = 60;
pub const MAX_AUTOMATION_PRECHECK_TIMEOUT_SECONDS: u32 = 600;
pub const MAX_AUTOMATION_PRECHECK_OUTPUT_CHARS: usize = 4_000;

pub const WEBHOOK_HOOK_ID_PREFIX: &str = "wh_";
pub const WEBHOOK_HOOK_ID_RANDOM_LEN: usize = 16;
pub const WEBHOOK_CALL_ID_PREFIX: &str = "call_";
pub const WEBHOOK_CALL_ID_RANDOM_LEN: usize = 20;
pub const WEBHOOK_RULE_ID_PREFIX: &str = "wh-";
pub const WEBHOOK_ACTOR_ID_PREFIX: &str = "webhook:";
pub const WEBHOOK_KEY_PREFIX: &str = "orcawh_";
pub const WEBHOOK_KEY_RANDOM_LEN: usize = 43;
pub const WEBHOOK_PATH_PREFIX: &str = "/hooks/";
pub const REDACTED_WEBHOOK_KEY: &str = "[orca webhook key]";

pub const WEBHOOK_MAX_WHERE: usize = 8;
pub const WEBHOOK_MAX_VALUES: usize = 10;
pub const WEBHOOK_MAX_PICK: usize = 12;
pub const WEBHOOK_MAX_PATH_LENGTH: usize = 200;
pub const WEBHOOK_MAX_VALUE_LENGTH: usize = 200;
pub const WEBHOOK_MAX_SUBJECT_LENGTH: usize = 256;
pub const WEBHOOK_RULE_MAX_BYTES: usize = 2_560;
pub const WEBHOOK_BODY_MAX_DEPTH: usize = 128;
pub const WEBHOOK_BODY_MAX_NODES: usize = 100_000;
pub const WEBHOOK_ENVELOPE_MAX_BODY_BYTES: usize = 1_048_576;
pub const WEBHOOK_ENVELOPE_MAX_HEADER_VALUE_LEN: usize = 1_024;
pub const WEBHOOK_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
pub const WEBHOOK_MAX_ARRAY_INDEX: usize = 999_999;

pub const WEBHOOK_CAP_WINDOW_MS: u64 = 3_600_000;
pub const WEBHOOK_REFUSAL_BURST_THRESHOLD: u32 = 5;
pub const WEBHOOK_REFUSAL_BURST_WINDOW_MS: u64 = 3_600_000;
pub const WEBHOOK_NOTICE_COOLDOWN_MS: u64 = 3_600_000;
pub const WEBHOOK_LEDGER_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const WEBHOOK_STARTS_AT_ONCE_PER_BOT: usize = 4;

pub const WAIT_EXPIRY_MS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const WAIT_MAX_DEADLINE_MS: u64 = WAIT_EXPIRY_MS * 12;

pub const WATCH_PR_DEFAULT_MS: u64 = 2 * 60 * 60 * 1000;
pub const WATCH_PR_MAX_FIX_ATTEMPTS: u8 = 2;

// ---------------------------------------------------------------------------
// 31 Event Trigger Kinds Across 6 Sources
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationEventSource {
    Slack,
    Github,
    Linear,
    Gmail,
    Calendar,
    Webhook,
}

impl AutomationEventSource {
    pub const ALL: [Self; 6] = [
        Self::Slack,
        Self::Github,
        Self::Linear,
        Self::Gmail,
        Self::Calendar,
        Self::Webhook,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Github => "github",
            Self::Linear => "linear",
            Self::Gmail => "gmail",
            Self::Calendar => "calendar",
            Self::Webhook => "webhook",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub const fn is_connector_source(self) -> bool {
        matches!(self, Self::Slack | Self::Github | Self::Linear)
    }

    pub const fn is_google_source(self) -> bool {
        matches!(self, Self::Gmail | Self::Calendar)
    }

    pub const fn is_host_local_only(self) -> bool {
        self.is_google_source()
    }

    pub const fn is_webhook_source(self) -> bool {
        matches!(self, Self::Webhook)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackEventKind {
    Mentioned,
    Message,
    Reaction,
}

impl SlackEventKind {
    pub const ALL: [Self; 3] = [Self::Mentioned, Self::Message, Self::Reaction];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mentioned => "mentioned",
            Self::Message => "message",
            Self::Reaction => "reaction",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubEventKind {
    IssueOpened,
    PrOpened,
    PrPushed,
    PrMerged,
    Comment,
    ReviewComment,
    Review,
    Mentioned,
    ReviewRequested,
    Assigned,
    CiFailed,
    CiPassed,
    CiFinished,
    PrConflicted,
    PrClosed,
    PrReady,
    Labeled,
    IssueClosed,
    DeployFinished,
    SecurityAlert,
    WorkflowFinished,
}

impl GitHubEventKind {
    pub const ALL: [Self; 21] = [
        Self::IssueOpened,
        Self::PrOpened,
        Self::PrPushed,
        Self::PrMerged,
        Self::Comment,
        Self::ReviewComment,
        Self::Review,
        Self::Mentioned,
        Self::ReviewRequested,
        Self::Assigned,
        Self::CiFailed,
        Self::CiPassed,
        Self::CiFinished,
        Self::PrConflicted,
        Self::PrClosed,
        Self::PrReady,
        Self::Labeled,
        Self::IssueClosed,
        Self::DeployFinished,
        Self::SecurityAlert,
        Self::WorkflowFinished,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueOpened => "issue_opened",
            Self::PrOpened => "pr_opened",
            Self::PrPushed => "pr_pushed",
            Self::PrMerged => "pr_merged",
            Self::Comment => "comment",
            Self::ReviewComment => "review_comment",
            Self::Review => "review",
            Self::Mentioned => "mentioned",
            Self::ReviewRequested => "review_requested",
            Self::Assigned => "assigned",
            Self::CiFailed => "ci_failed",
            Self::CiPassed => "ci_passed",
            Self::CiFinished => "ci_finished",
            Self::PrConflicted => "pr_conflicted",
            Self::PrClosed => "pr_closed",
            Self::PrReady => "pr_ready",
            Self::Labeled => "labeled",
            Self::IssueClosed => "issue_closed",
            Self::DeployFinished => "deploy_finished",
            Self::SecurityAlert => "security_alert",
            Self::WorkflowFinished => "workflow_finished",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinearEventKind {
    Assigned,
    Mentioned,
    IssueCreated,
    IssueMoved,
}

impl LinearEventKind {
    pub const ALL: [Self; 4] = [
        Self::Assigned,
        Self::Mentioned,
        Self::IssueCreated,
        Self::IssueMoved,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Mentioned => "mentioned",
            Self::IssueCreated => "issue_created",
            Self::IssueMoved => "issue_moved",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GmailEventKind {
    NewEmail,
}

impl GmailEventKind {
    pub const ALL: [Self; 1] = [Self::NewEmail];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NewEmail => "new_email",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarEventKind {
    StartingSoon,
}

impl CalendarEventKind {
    pub const ALL: [Self; 1] = [Self::StartingSoon];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartingSoon => "starting_soon",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEventKind {
    Call,
}

impl WebhookEventKind {
    pub const ALL: [Self; 1] = [Self::Call];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", content = "event", rename_all = "snake_case")]
pub enum AutomationEventKind {
    Slack(SlackEventKind),
    Github(GitHubEventKind),
    Linear(LinearEventKind),
    Gmail(GmailEventKind),
    Calendar(CalendarEventKind),
    Webhook(WebhookEventKind),
}

impl AutomationEventKind {
    pub const ALL: [Self; 31] = [
        Self::Slack(SlackEventKind::Mentioned),
        Self::Slack(SlackEventKind::Message),
        Self::Slack(SlackEventKind::Reaction),
        Self::Github(GitHubEventKind::IssueOpened),
        Self::Github(GitHubEventKind::PrOpened),
        Self::Github(GitHubEventKind::PrPushed),
        Self::Github(GitHubEventKind::PrMerged),
        Self::Github(GitHubEventKind::Comment),
        Self::Github(GitHubEventKind::ReviewComment),
        Self::Github(GitHubEventKind::Review),
        Self::Github(GitHubEventKind::Mentioned),
        Self::Github(GitHubEventKind::ReviewRequested),
        Self::Github(GitHubEventKind::Assigned),
        Self::Github(GitHubEventKind::CiFailed),
        Self::Github(GitHubEventKind::CiPassed),
        Self::Github(GitHubEventKind::CiFinished),
        Self::Github(GitHubEventKind::PrConflicted),
        Self::Github(GitHubEventKind::PrClosed),
        Self::Github(GitHubEventKind::PrReady),
        Self::Github(GitHubEventKind::Labeled),
        Self::Github(GitHubEventKind::IssueClosed),
        Self::Github(GitHubEventKind::DeployFinished),
        Self::Github(GitHubEventKind::SecurityAlert),
        Self::Github(GitHubEventKind::WorkflowFinished),
        Self::Linear(LinearEventKind::Assigned),
        Self::Linear(LinearEventKind::Mentioned),
        Self::Linear(LinearEventKind::IssueCreated),
        Self::Linear(LinearEventKind::IssueMoved),
        Self::Gmail(GmailEventKind::NewEmail),
        Self::Calendar(CalendarEventKind::StartingSoon),
        Self::Webhook(WebhookEventKind::Call),
    ];

    pub const fn source(self) -> AutomationEventSource {
        match self {
            Self::Slack(_) => AutomationEventSource::Slack,
            Self::Github(_) => AutomationEventSource::Github,
            Self::Linear(_) => AutomationEventSource::Linear,
            Self::Gmail(_) => AutomationEventSource::Gmail,
            Self::Calendar(_) => AutomationEventSource::Calendar,
            Self::Webhook(_) => AutomationEventSource::Webhook,
        }
    }

    pub const fn event_str(self) -> &'static str {
        match self {
            Self::Slack(e) => e.as_str(),
            Self::Github(e) => e.as_str(),
            Self::Linear(e) => e.as_str(),
            Self::Gmail(e) => e.as_str(),
            Self::Calendar(e) => e.as_str(),
            Self::Webhook(e) => e.as_str(),
        }
    }

    pub fn parse(source: &str, event: &str) -> Option<Self> {
        match AutomationEventSource::parse(source)? {
            AutomationEventSource::Slack => SlackEventKind::parse(event).map(Self::Slack),
            AutomationEventSource::Github => GitHubEventKind::parse(event).map(Self::Github),
            AutomationEventSource::Linear => LinearEventKind::parse(event).map(Self::Linear),
            AutomationEventSource::Gmail => GmailEventKind::parse(event).map(Self::Gmail),
            AutomationEventSource::Calendar => CalendarEventKind::parse(event).map(Self::Calendar),
            AutomationEventSource::Webhook => WebhookEventKind::parse(event).map(Self::Webhook),
        }
    }

    /// `from_owner: true` is permitted ONLY on `github.pr_opened`.
    pub const fn takes_own_events(self) -> bool {
        matches!(self, Self::Github(GitHubEventKind::PrOpened))
    }

    pub const fn is_host_local_only(self) -> bool {
        self.source().is_host_local_only()
    }

    pub const fn is_pr_state_event(self) -> bool {
        matches!(
            self,
            Self::Github(
                GitHubEventKind::CiPassed
                    | GitHubEventKind::CiFinished
                    | GitHubEventKind::PrConflicted
            )
        )
    }

    pub const fn needs_github2(self, has_label_filter: bool) -> bool {
        match self {
            Self::Github(e) => {
                has_label_filter
                    || matches!(
                        e,
                        GitHubEventKind::CiPassed
                            | GitHubEventKind::PrConflicted
                            | GitHubEventKind::PrClosed
                            | GitHubEventKind::PrReady
                            | GitHubEventKind::Labeled
                            | GitHubEventKind::IssueClosed
                            | GitHubEventKind::DeployFinished
                            | GitHubEventKind::SecurityAlert
                    )
            }
            _ => false,
        }
    }

    pub const fn needs_workflows(self) -> bool {
        matches!(self, Self::Github(GitHubEventKind::WorkflowFinished))
    }

    pub const fn needs_ci_finished(self) -> bool {
        matches!(self, Self::Github(GitHubEventKind::CiFinished))
    }
}

// ---------------------------------------------------------------------------
// AutomationBotTarget, Precheck & EVENT_ONLY_SCHEDULE
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationBotRunsIn {
    New,
    Chat {
        #[serde(alias = "sessionId")]
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, alias = "homeIfGone", skip_serializing_if = "Option::is_none")]
        home_if_gone: Option<bool>,
        #[serde(default, alias = "toConversation", skip_serializing_if = "Option::is_none")]
        to_conversation: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationBotResultsTo {
    Chat,
    Channel {
        channel: String,
        #[serde(default, alias = "refId", skip_serializing_if = "Option::is_none")]
        ref_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    LinearIssue {
        #[serde(alias = "linearIssue")]
        linear_issue: String,
        #[serde(default, alias = "refId", skip_serializing_if = "Option::is_none")]
        ref_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    None,
    Thread,
}

impl AutomationBotResultsTo {
    pub fn display_place(&self) -> Option<&str> {
        match self {
            Self::Channel { channel, label, .. } => label.as_deref().or(Some(channel.as_str())),
            Self::LinearIssue {
                linear_issue,
                label,
                ..
            } => label.as_deref().or(Some(linear_issue.as_str())),
            Self::Chat | Self::None | Self::Thread => None,
        }
    }

    pub fn validate(&self) -> Result<(), BotError> {
        match self {
            Self::Channel { channel, .. } if channel.trim().is_empty() => Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "results_to channel must be non-empty.",
            )),
            Self::LinearIssue { linear_issue, .. } if linear_issue.trim().is_empty() => {
                Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "results_to linear_issue must be non-empty.",
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationPrecheck {
    pub command: String,
    #[serde(alias = "timeoutSeconds")]
    pub timeout_seconds: u32,
}

/// Trims `command` (returning `None` if empty) and clamps `timeout_seconds` into `1..=600`
/// (defaulting to `60` if `None`).
pub fn normalize_automation_precheck(
    command: &str,
    timeout_seconds: Option<u32>,
) -> Option<AutomationPrecheck> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let timeout = timeout_seconds
        .unwrap_or(DEFAULT_AUTOMATION_PRECHECK_TIMEOUT_SECONDS)
        .clamp(1, MAX_AUTOMATION_PRECHECK_TIMEOUT_SECONDS);
    Some(AutomationPrecheck {
        command: trimmed.to_string(),
        timeout_seconds: timeout,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationBotTarget {
    #[serde(alias = "botId")]
    pub bot_id: String,
    #[serde(alias = "runsIn")]
    pub runs_in: AutomationBotRunsIn,
    #[serde(alias = "resultsTo")]
    pub results_to: AutomationBotResultsTo,
    #[serde(default, alias = "agentSetup", skip_serializing_if = "Option::is_none")]
    pub agent_setup: Option<crate::bot_types::BotLaunchConfig>,
    #[serde(default, alias = "letBotReact", skip_serializing_if = "Option::is_none")]
    pub let_bot_react: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precheck: Option<AutomationPrecheck>,
    #[serde(default, alias = "setupTrustHash", skip_serializing_if = "Option::is_none")]
    pub setup_trust_hash: Option<String>,
    #[serde(default, alias = "missedRunGraceMinutes", skip_serializing_if = "Option::is_none")]
    pub missed_run_grace_minutes: Option<u32>,
}

impl AutomationBotTarget {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.bot_id.trim().is_empty() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Automation bot target requires a non-empty bot_id.",
            ));
        }
        if let AutomationBotRunsIn::Chat { ref session_id, .. } = self.runs_in
            && session_id.trim().is_empty()
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Automation bot target `runs_in = chat` requires a non-empty session_id.",
            ));
        }
        self.results_to.validate()
    }
}

pub const fn has_automation_schedule(dtstart_ms: i64) -> bool {
    dtstart_ms < EVENT_ONLY_SCHEDULE_DTSTART_MS
}

pub fn webhook_is_only_start(dtstart_ms: i64, triggers: &[AutomationEventTrigger]) -> bool {
    !has_automation_schedule(dtstart_ms)
        && !triggers.is_empty()
        && triggers
            .iter()
            .all(|t| t.source() == AutomationEventSource::Webhook)
}

// ---------------------------------------------------------------------------
// Trigger Structs, Validation & Event Folding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationConnectorEventTrigger {
    pub id: String,
    pub kind: AutomationEventKind,
    #[serde(rename = "in", alias = "in_places", alias = "in_channels", default)]
    pub in_places: Vec<String>,
    #[serde(default)]
    pub from: Vec<String>,
    #[serde(default, alias = "textContains", skip_serializing_if = "Option::is_none")]
    pub text_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, alias = "onOwnerMessage", skip_serializing_if = "Option::is_none")]
    pub on_owner_message: Option<bool>,
    #[serde(default, alias = "fromOwner", skip_serializing_if = "Option::is_none")]
    pub from_owner: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AutomationGoogleTrigger {
    Gmail {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(default)]
        from: Vec<String>,
        #[serde(default)]
        to: Vec<String>,
        #[serde(default, alias = "subjectContains", skip_serializing_if = "Option::is_none")]
        subject_contains: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Calendar {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        calendar: Option<String>,
        #[serde(alias = "minutesBefore")]
        minutes_before: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationWebhookTrigger {
    pub id: String,
    pub rev: u32,
    #[serde(default)]
    pub filters: WebhookFilters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "triggerKind", rename_all = "snake_case")]
pub enum AutomationEventTrigger {
    Connector(AutomationConnectorEventTrigger),
    Google(AutomationGoogleTrigger),
    Webhook(AutomationWebhookTrigger),
}

impl AutomationEventTrigger {
    pub fn id(&self) -> &str {
        match self {
            Self::Connector(c) => &c.id,
            Self::Google(AutomationGoogleTrigger::Gmail { id, .. })
            | Self::Google(AutomationGoogleTrigger::Calendar { id, .. }) => id,
            Self::Webhook(w) => &w.id,
        }
    }

    pub const fn source(&self) -> AutomationEventSource {
        match self {
            Self::Connector(c) => c.kind.source(),
            Self::Google(AutomationGoogleTrigger::Gmail { .. }) => AutomationEventSource::Gmail,
            Self::Google(AutomationGoogleTrigger::Calendar { .. }) => {
                AutomationEventSource::Calendar
            }
            Self::Webhook(_) => AutomationEventSource::Webhook,
        }
    }

    pub const fn kind(&self) -> AutomationEventKind {
        match self {
            Self::Connector(c) => c.kind,
            Self::Google(AutomationGoogleTrigger::Gmail { .. }) => {
                AutomationEventKind::Gmail(GmailEventKind::NewEmail)
            }
            Self::Google(AutomationGoogleTrigger::Calendar { .. }) => {
                AutomationEventKind::Calendar(CalendarEventKind::StartingSoon)
            }
            Self::Webhook(_) => AutomationEventKind::Webhook(WebhookEventKind::Call),
        }
    }

    pub const fn is_host_local_only(&self) -> bool {
        self.source().is_host_local_only()
    }
}

fn is_ascii_digits_in_range(s: &str, min_len: usize, max_len: usize) -> bool {
    let len = s.len();
    len >= min_len && len <= max_len && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_resolved_github_workflow_place(place: &str) -> bool {
    let Some(rest) = place.strip_prefix("github:") else {
        return false;
    };
    let Some((inst, repo)) = rest.split_once(':') else {
        return false;
    };
    !repo.contains(':') && is_ascii_digits_in_range(inst, 1, 32) && is_ascii_digits_in_range(repo, 1, 32)
}

/// Sanitizes optional display-name hints on a connector trigger (trims & caps to 200 chars).
pub fn with_trigger_names(raw: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        let tk = k.trim();
        let tv = v.trim();
        if !tk.is_empty() && !tv.is_empty() {
            let capped: String = tv.chars().take(MAX_TRIGGER_FILTER_LENGTH).collect();
            out.insert(tk.to_string(), capped);
        }
    }
    out
}

pub fn validate_connector_trigger(
    t: &AutomationConnectorEventTrigger,
    once: Option<&OneShotWait>,
) -> Result<(), BotError> {
    let id_len = t.id.trim().chars().count();
    if id_len == 0 || id_len > MAX_TRIGGER_ID_LENGTH {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Trigger id must be 1..={MAX_TRIGGER_ID_LENGTH} characters."),
        ));
    }
    if !t.kind.source().is_connector_source() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Connector event trigger must have source slack, github, or linear.",
        ));
    }
    if t.in_places.len() > MAX_TRIGGER_PLACES || t.from.len() > MAX_TRIGGER_PLACES {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Trigger `in` and `from` filters cannot exceed {MAX_TRIGGER_PLACES} entries."),
        ));
    }
    for entry in t.in_places.iter().chain(t.from.iter()) {
        let l = entry.trim().chars().count();
        if l == 0 || l > MAX_TRIGGER_ID_LENGTH {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Trigger place/sender entry must be 1..={MAX_TRIGGER_ID_LENGTH} characters."),
            ));
        }
    }
    for opt_str in [
        t.text_contains.as_deref(),
        t.subject.as_deref(),
        t.label.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let l = opt_str.trim().chars().count();
        if l == 0 || l > MAX_TRIGGER_FILTER_LENGTH {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Trigger filter text must be 1..={MAX_TRIGGER_FILTER_LENGTH} characters."),
            ));
        }
    }

    if t.from_owner == Some(true) && !t.kind.takes_own_events() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "`from_owner: true` is only permitted on github.pr_opened (got {}.{}).",
                t.kind.source().as_str(),
                t.kind.event_str()
            ),
        ));
    }
    if t.on_owner_message == Some(true)
        && !matches!(t.kind, AutomationEventKind::Slack(SlackEventKind::Reaction))
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "`on_owner_message: true` is only permitted on slack.reaction.",
        ));
    }
    if t.label.is_some() && !matches!(t.kind, AutomationEventKind::Github(GitHubEventKind::Labeled))
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "`label` filter is only permitted on github.labeled.",
        ));
    }
    if t.run.is_some()
        && !matches!(
            t.kind,
            AutomationEventKind::Github(GitHubEventKind::WorkflowFinished)
        )
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "`run` filter is only permitted on github.workflow_finished.",
        ));
    }

    if matches!(
        t.kind,
        AutomationEventKind::Github(GitHubEventKind::WorkflowFinished)
    ) {
        let Some(ref run_id) = t.run else {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "github.workflow_finished requires a numeric `run` id (1..=20 digits).",
            ));
        };
        if !is_ascii_digits_in_range(run_id, 1, 20) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "github.workflow_finished `run` must match ^\\d{1,20}$.",
            ));
        }
        if t.in_places.len() != 1 || !is_resolved_github_workflow_place(&t.in_places[0]) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "github.workflow_finished requires exactly 1 resolved repo `github:<inst>:<repo>`.",
            ));
        }
        if t.subject.is_some() || !t.from.is_empty() || t.from_owner == Some(true) || t.on_owner_message == Some(true) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "github.workflow_finished forbids `subject`, `from`, `from_owner`, and `on_owner_message`.",
            ));
        }
        if once.and_then(|o| o.by).is_none() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "github.workflow_finished requires a one-shot wait with a deadline (`once.by`).",
            ));
        }
    }

    if t.kind.is_pr_state_event() {
        if t.subject.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "github.{} requires a non-empty `subject`.",
                    t.kind.event_str()
                ),
            ));
        }
        if !t.from.is_empty() || t.from_owner == Some(true) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "github.{} forbids `from` and `from_owner`.",
                    t.kind.event_str()
                ),
            ));
        }
    }

    Ok(())
}

pub fn validate_google_trigger(t: &AutomationGoogleTrigger) -> Result<(), BotError> {
    match t {
        AutomationGoogleTrigger::Gmail {
            id,
            account,
            from,
            to,
            subject_contains,
            label,
        } => {
            let id_len = id.trim().chars().count();
            if id_len == 0 || id_len > MAX_TRIGGER_ID_LENGTH {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Gmail trigger id must be 1..=128 characters.",
                ));
            }
            if from.len() > MAX_GOOGLE_ADDRESSES || to.len() > MAX_GOOGLE_ADDRESSES {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!("Gmail trigger `from`/`to` lists cannot exceed {MAX_GOOGLE_ADDRESSES}."),
                ));
            }
            for s in account
                .iter()
                .chain(subject_contains.iter())
                .chain(label.iter())
                .chain(from.iter())
                .chain(to.iter())
            {
                let l = s.trim().chars().count();
                if l == 0 || l > MAX_GOOGLE_FILTER_LENGTH {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!(
                            "Gmail trigger filter must be 1..={MAX_GOOGLE_FILTER_LENGTH} characters."
                        ),
                    ));
                }
            }
            Ok(())
        }
        AutomationGoogleTrigger::Calendar {
            id,
            account,
            calendar,
            minutes_before,
        } => {
            let id_len = id.trim().chars().count();
            if id_len == 0 || id_len > MAX_TRIGGER_ID_LENGTH {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Calendar trigger id must be 1..=128 characters.",
                ));
            }
            if *minutes_before > CALENDAR_MAX_MINUTES_BEFORE {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Calendar `minutes_before` must be in 0..={CALENDAR_MAX_MINUTES_BEFORE}."
                    ),
                ));
            }
            for s in account.iter().chain(calendar.iter()) {
                let l = s.trim().chars().count();
                if l == 0 || l > MAX_GOOGLE_FILTER_LENGTH {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!(
                            "Calendar trigger filter must be 1..={MAX_GOOGLE_FILTER_LENGTH} characters."
                        ),
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Validates an automation's full event trigger list and cross-cutting invariants
/// (`MAX_AUTOMATION_EVENT_TRIGGERS = 20`, unique IDs, max 1 webhook trigger, `from_owner`
/// exclusivity, `workflow_finished` + `once.by`, `results_to == Thread` guards).
pub fn validate_automation_event_triggers(
    triggers: &[AutomationEventTrigger],
    bot: Option<&AutomationBotTarget>,
    dtstart_ms: i64,
    once: Option<&OneShotWait>,
) -> Result<(), BotError> {
    if triggers.len() > MAX_AUTOMATION_EVENT_TRIGGERS {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "An automation cannot have more than {MAX_AUTOMATION_EVENT_TRIGGERS} event triggers."
            ),
        ));
    }
    let has_schedule = has_automation_schedule(dtstart_ms);
    if !triggers.is_empty() && bot.is_none() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Event triggers require a bot target.",
        ));
    }
    if let Some(wait) = once {
        if bot.is_none() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "One-shot wait automations require a bot target.",
            ));
        }
        if has_schedule {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "One-shot wait automations cannot also have a recurring schedule.",
            ));
        }
        if triggers.is_empty() && wait.by.is_none() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "One-shot wait automation requires at least one event trigger or a deadline (`once.by`).",
            ));
        }
    } else if !has_schedule && triggers.is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Automation requires either a schedule or at least one event trigger.",
        ));
    }

    let mut seen_ids: HashSet<&str> = HashSet::with_capacity(triggers.len());
    let mut webhook_count = 0usize;
    let mut has_google = false;

    for trigger in triggers {
        let tid = trigger.id().trim();
        if tid.is_empty() || !seen_ids.insert(tid) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Trigger IDs must be non-empty and unique within an automation.",
            ));
        }
        match trigger {
            AutomationEventTrigger::Connector(c) => validate_connector_trigger(c, once)?,
            AutomationEventTrigger::Google(g) => {
                has_google = true;
                validate_google_trigger(g)?;
            }
            AutomationEventTrigger::Webhook(w) => {
                webhook_count += 1;
                if webhook_count > 1 {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        "An automation can have at most 1 webhook trigger.",
                    ));
                }
                if hook_id_of_rule(&w.id).is_none() || w.rev == 0 {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        "Webhook trigger requires valid `wh-wh_<16>` rule id and rev >= 1.",
                    ));
                }
                validate_webhook_filters(&w.filters)?;
            }
        }
    }

    if let Some(target) = bot {
        target.validate()?;
        if target.results_to == AutomationBotResultsTo::Thread
            && (has_schedule
                || triggers.is_empty()
                || webhook_count > 0
                || has_google
                || target.runs_in != AutomationBotRunsIn::New)
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "`results_to = thread` requires event-only schedule, connector triggers only (no webhook/google), and `runs_in = new`.",
            ));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldedEventEntry {
    pub key: String,
    #[serde(alias = "occurredAt")]
    pub occurred_at: u64,
}

pub fn run_has_event(latest_event_key: Option<&str>, folded_keys: &[String], key: &str) -> bool {
    latest_event_key == Some(key) || folded_keys.iter().any(|k| k == key)
}

/// Folds `incoming` into `current`, keeping whichever has strictly greater `occurred_at`
/// as the latest event and retaining at most the last `MAX_RUN_EVENT_KEYS` (`50`) folded keys.
pub fn with_folded_event(
    current: FoldedEventEntry,
    mut folded_keys: Vec<String>,
    incoming: FoldedEventEntry,
) -> (FoldedEventEntry, Vec<String>) {
    let (latest, folded_key) = if current.occurred_at > incoming.occurred_at {
        (current, incoming.key)
    } else {
        (incoming, current.key)
    };
    folded_keys.retain(|k| k != &latest.key);
    if !folded_keys.iter().any(|k| k == &folded_key) && folded_key != latest.key {
        folded_keys.push(folded_key);
    }
    if folded_keys.len() > MAX_RUN_EVENT_KEYS {
        let drop_n = folded_keys.len() - MAX_RUN_EVENT_KEYS;
        folded_keys.drain(0..drop_n);
    }
    (latest, folded_keys)
}

// ---------------------------------------------------------------------------
// 6-Adapter Webhook Contract, ID/Key Validators & Linear-Time Key Redactor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookAdapterId {
    #[default]
    Standard,
    Github,
    Sentry,
    Linear,
    Pagerduty,
    Stripe,
}

impl WebhookAdapterId {
    pub const ALL: [Self; 6] = [
        Self::Standard,
        Self::Github,
        Self::Sentry,
        Self::Linear,
        Self::Pagerduty,
        Self::Stripe,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Github => "github",
            Self::Sentry => "sentry",
            Self::Linear => "linear",
            Self::Pagerduty => "pagerduty",
            Self::Stripe => "stripe",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == raw)
    }
}

impl fmt::Display for WebhookAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookReceiptStatus {
    Queued,
    Ignored,
    Duplicate,
    Paused,
}

/// CLI-safe webhook summary: strictly omits raw secret key and raw webhook URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookCliSummary {
    pub hook_id: String,
    pub adapter: WebhookAdapterId,
    pub rev: u32,
    pub epoch: u64,
    pub paused: bool,
    pub filters: WebhookFilters,
}

pub fn is_valid_webhook_hook_id(id: &str) -> bool {
    id.len() == WEBHOOK_HOOK_ID_PREFIX.len() + WEBHOOK_HOOK_ID_RANDOM_LEN
        && id.starts_with(WEBHOOK_HOOK_ID_PREFIX)
        && id
            .as_bytes()
            .get(WEBHOOK_HOOK_ID_PREFIX.len()..)
            .is_some_and(|rest| rest.iter().all(u8::is_ascii_alphanumeric))
}

pub fn is_valid_webhook_call_id(id: &str) -> bool {
    id.len() == WEBHOOK_CALL_ID_PREFIX.len() + WEBHOOK_CALL_ID_RANDOM_LEN
        && id.starts_with(WEBHOOK_CALL_ID_PREFIX)
        && id
            .as_bytes()
            .get(WEBHOOK_CALL_ID_PREFIX.len()..)
            .is_some_and(|rest| rest.iter().all(u8::is_ascii_alphanumeric))
}

pub fn is_valid_webhook_key(key: &str) -> bool {
    key.len() == WEBHOOK_KEY_PREFIX.len() + WEBHOOK_KEY_RANDOM_LEN
        && key.starts_with(WEBHOOK_KEY_PREFIX)
        && key.as_bytes().get(WEBHOOK_KEY_PREFIX.len()..).is_some_and(|rest| {
            rest.iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
        })
}

pub fn webhook_rule_id(hook_id: &str) -> Result<String, BotError> {
    if !is_valid_webhook_hook_id(hook_id) {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid webhook hook id '{hook_id}'."),
        ));
    }
    Ok(format!("{WEBHOOK_RULE_ID_PREFIX}{hook_id}"))
}

pub fn hook_id_of_rule(rule_id: &str) -> Option<&str> {
    let rest = rule_id.strip_prefix(WEBHOOK_RULE_ID_PREFIX)?;
    is_valid_webhook_hook_id(rest).then_some(rest)
}

pub fn webhook_actor_id(hook_id: &str) -> Result<String, BotError> {
    if !is_valid_webhook_hook_id(hook_id) {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid webhook hook id '{hook_id}'."),
        ));
    }
    Ok(format!("{WEBHOOK_ACTOR_ID_PREFIX}{hook_id}"))
}

pub fn hook_id_of_actor(actor_id: &str) -> Option<&str> {
    let rest = actor_id.strip_prefix(WEBHOOK_ACTOR_ID_PREFIX)?;
    is_valid_webhook_hook_id(rest).then_some(rest)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn consume_encoded_ascii(bytes: &[u8], pos: usize) -> Option<(u8, usize)> {
    let b = *bytes.get(pos)?;
    if b == b'\\' {
        if pos + 6 <= bytes.len()
            && bytes[pos + 1] == b'u'
            && bytes[pos + 2] == b'0'
            && bytes[pos + 3] == b'0'
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[pos + 4]), hex_nibble(bytes[pos + 5]))
        {
            let val = (hi << 4) | lo;
            if val < 0x80 {
                return Some((val, pos + 6));
            }
        }
        if let Some(&next) = bytes.get(pos + 1)
            && matches!(next, b'_' | b'-')
        {
            return Some((next, pos + 2));
        }
        return None;
    }
    if b == b'%'
        && pos + 3 <= bytes.len()
        && let (Some(hi), Some(lo)) = (hex_nibble(bytes[pos + 1]), hex_nibble(bytes[pos + 2]))
    {
        let val = (hi << 4) | lo;
        if val < 0x80 {
            return Some((val, pos + 3));
        }
        return None;
    }
    (b < 0x80).then_some((b, pos + 1))
}

fn match_encoded_orcawh_key(bytes: &[u8], start: usize) -> Option<usize> {
    const PREFIX: &[u8] = b"orcawh_";
    let mut cur = start;
    for &expected in PREFIX {
        let (decoded, next) = consume_encoded_ascii(bytes, cur)?;
        if decoded != expected {
            return None;
        }
        cur = next;
    }
    let mut key_chars = 0usize;
    while let Some((decoded, next)) = consume_encoded_ascii(bytes, cur) {
        if decoded.is_ascii_alphanumeric() || decoded == b'_' || decoded == b'-' {
            key_chars += 1;
            cur = next;
        } else {
            break;
        }
    }
    (key_chars > 0).then_some(cur)
}

fn is_url_key_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '/' | '?' | '#' | '"' | '\'' | '<' | '>' | ')' | ']')
}

/// Linear-time single-pass webhook key redactor: replaces plain, Markdown-escaped (`\_`),
/// percent-encoded (`%XX`), and `\u00XX`-escaped `orcawh_...` keys AND secret path tails
/// after `/hooks/wh_<16 alnum>/` with `"[orca webhook key]"`.
pub fn redact_webhook_keys(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0usize;

    while i < len {
        // 1. `/hooks/wh_<16 alnum>/<secret>`
        let hook_prefix = "/hooks/wh_";
        let min_url_len = hook_prefix.len() + WEBHOOK_HOOK_ID_RANDOM_LEN + 1;
        if i + min_url_len < len
            && text.get(i..).is_some_and(|s| s.starts_with(hook_prefix))
            && bytes[i + hook_prefix.len()..i + hook_prefix.len() + WEBHOOK_HOOK_ID_RANDOM_LEN]
                .iter()
                .all(u8::is_ascii_alphanumeric)
            && bytes[i + hook_prefix.len() + WEBHOOK_HOOK_ID_RANDOM_LEN] == b'/'
        {
            let key_start = i + min_url_len;
            if let Some(rest) = text.get(key_start..) {
                if let Some(after_redacted) = rest.strip_prefix(REDACTED_WEBHOOK_KEY) {
                    let extra_len = after_redacted
                        .char_indices()
                        .find(|&(_, ch)| is_url_key_delimiter(ch))
                        .map_or(after_redacted.len(), |(idx, _)| idx);
                    if let Some(prefix_slice) = text.get(i..key_start) {
                        out.push_str(prefix_slice);
                    }
                    out.push_str(REDACTED_WEBHOOK_KEY);
                    i = key_start + REDACTED_WEBHOOK_KEY.len() + extra_len;
                    continue;
                }
                let key_len = rest
                    .char_indices()
                    .find(|&(_, ch)| is_url_key_delimiter(ch))
                    .map_or(rest.len(), |(idx, _)| idx);
                if key_len > 0 {
                    if let Some(prefix_slice) = text.get(i..key_start) {
                        out.push_str(prefix_slice);
                    }
                    out.push_str(REDACTED_WEBHOOK_KEY);
                    i = key_start + key_len;
                    continue;
                }
            }
        }

        // 2. Plain / escaped / percent / \u00XX encoded `orcawh_...`
        if matches!(bytes[i], b'o' | b'%' | b'\\')
            && let Some(end) = match_encoded_orcawh_key(bytes, i)
        {
            out.push_str(REDACTED_WEBHOOK_KEY);
            i = end;
            continue;
        }

        if let Some(ch) = text.get(i..).and_then(|s| s.chars().next()) {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    out
}

// ---------------------------------------------------------------------------
// 6 JSON-Path Filter Ops, Safe Path Parser/Resolver & Rule Validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebhookWhereOp {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    NotEq,
    #[serde(rename = "in")]
    In,
    #[serde(rename = "contains")]
    Contains,
    #[serde(rename = "exists")]
    Exists,
    #[serde(rename = "!exists")]
    NotExists,
}

impl WebhookWhereOp {
    pub const ALL: [Self; 6] = [
        Self::Eq,
        Self::NotEq,
        Self::In,
        Self::Contains,
        Self::Exists,
        Self::NotExists,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::NotEq => "!=",
            Self::In => "in",
            Self::Contains => "contains",
            Self::Exists => "exists",
            Self::NotExists => "!exists",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookWhereClause {
    pub path: String,
    pub op: WebhookWhereOp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookFilters {
    #[serde(rename = "where", default, skip_serializing_if = "Vec::is_empty")]
    pub where_clauses: Vec<WebhookWhereClause>,
    #[serde(default, alias = "text_contains", skip_serializing_if = "Option::is_none")]
    pub text_contains: Option<String>,
    #[serde(default, alias = "subject_from", skip_serializing_if = "Option::is_none")]
    pub subject_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pick: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRuleWire {
    pub id: String,
    pub automation_id: String,
    pub bot_id: String,
    pub source: AutomationEventSource,
    pub event: WebhookEventKind,
    pub rev: u32,
    #[serde(default)]
    pub filters: WebhookFilters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_minutes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookPathStep {
    Key(String),
    Index(usize),
}

const FORBIDDEN_PATH_SEGMENTS: [&str; 3] = ["__proto__", "prototype", "constructor"];

/// Pure-Rust safe JSON-path parser (`body` / `headers` root, `[0..=999999]`, forbids
/// `__proto__`/`prototype`/`constructor`, lowercases `headers.<key>`, max 200 chars).
pub fn parse_webhook_path(path: &str) -> Result<Vec<WebhookPathStep>, BotError> {
    if path.is_empty() || path.chars().count() > WEBHOOK_MAX_PATH_LENGTH {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Webhook path must be 1..={WEBHOOK_MAX_PATH_LENGTH} characters."),
        ));
    }
    let mut steps: Vec<WebhookPathStep> = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Webhook path '{path}' contains an empty dot segment."),
            ));
        }
        let bracket_pos = part.find('[');
        let key_part = match bracket_pos {
            Some(idx) => part.get(..idx).ok_or_else(|| {
                BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!("Invalid key segment '{part}' in webhook path '{path}'."),
                )
            })?,
            None => part,
        };
        if key_part.is_empty()
            || key_part.contains(']')
            || key_part.chars().any(char::is_whitespace)
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Invalid key segment '{part}' in webhook path '{path}'."),
            ));
        }
        let key = if steps.len() == 1 && matches!(steps.first(), Some(WebhookPathStep::Key(r)) if r == "headers")
        {
            key_part.to_ascii_lowercase()
        } else {
            key_part.to_string()
        };
        if FORBIDDEN_PATH_SEGMENTS.contains(&key.as_str()) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Forbidden segment '{key_part}' in webhook path."),
            ));
        }
        steps.push(WebhookPathStep::Key(key));

        if let Some(mut pos) = bracket_pos {
            let pbytes = part.as_bytes();
            while pos < pbytes.len() {
                if pbytes[pos] != b'[' {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Unexpected trailing characters in webhook path '{path}'."),
                    ));
                }
                let after_open = part.get(pos + 1..).ok_or_else(|| {
                    BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Unclosed array index in webhook path '{path}'."),
                    )
                })?;
                let close_rel = after_open.find(']').ok_or_else(|| {
                    BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Unclosed array index in webhook path '{path}'."),
                    )
                })?;
                let idx_str = after_open.get(..close_rel).ok_or_else(|| {
                    BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Invalid array index in webhook path '{path}'."),
                    )
                })?;
                if idx_str.is_empty()
                    || idx_str.len() > 6
                    || !idx_str.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Invalid array index '[{idx_str}]' in webhook path '{path}'."),
                    ));
                }
                let idx = idx_str.parse::<usize>().map_err(|_| {
                    BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Array index overflow in webhook path '{path}'."),
                    )
                })?;
                if idx > WEBHOOK_MAX_ARRAY_INDEX {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Array index {idx} exceeds {WEBHOOK_MAX_ARRAY_INDEX}."),
                    ));
                }
                steps.push(WebhookPathStep::Index(idx));
                pos += 1 + close_rel + 1;
            }
        }
    }

    match steps.first() {
        Some(WebhookPathStep::Key(root)) if root == "body" || root == "headers" => Ok(steps),
        _ => Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Webhook path '{path}' must start with 'body' or 'headers'."),
        )),
    }
}

pub struct WebhookCallView<'a> {
    pub headers: &'a BTreeMap<String, String>,
    pub body: &'a Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedWebhookValue<'a> {
    Json(&'a Value),
    Header(&'a str),
    HeadersRoot,
}

/// Resolves a parsed JSON path against `call` (own-property-only, case-insensitive headers).
pub fn resolve_webhook_path<'a>(
    steps: &[WebhookPathStep],
    call: &WebhookCallView<'a>,
) -> Option<ResolvedWebhookValue<'a>> {
    match steps.first()? {
        WebhookPathStep::Key(root) if root == "headers" => {
            if steps.len() == 1 {
                return Some(ResolvedWebhookValue::HeadersRoot);
            }
            if steps.len() == 2
                && let WebhookPathStep::Key(hkey) = &steps[1]
            {
                let found = call.headers.get(hkey).or_else(|| {
                    call.headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(hkey))
                        .map(|(_, v)| v)
                });
                return found.map(|v| ResolvedWebhookValue::Header(v.as_str()));
            }
            None
        }
        WebhookPathStep::Key(root) if root == "body" => {
            let mut cur = call.body;
            for step in &steps[1..] {
                match step {
                    WebhookPathStep::Key(k) => {
                        cur = cur.as_object()?.get(k)?;
                    }
                    WebhookPathStep::Index(i) => {
                        cur = cur.as_array()?.get(*i)?;
                    }
                }
            }
            Some(ResolvedWebhookValue::Json(cur))
        }
        _ => None,
    }
}

/// Returns `Some(f64)` iff `n` is an exact finite number within `[-2^53+1, 2^53-1]`.
pub fn is_exact_json_number(n: &serde_json::Number) -> Option<f64> {
    let max_safe_u64 = WEBHOOK_MAX_SAFE_INTEGER as u64;
    if let Some(i) = n.as_i64() {
        return (i.unsigned_abs() <= max_safe_u64).then_some(i as f64);
    }
    if let Some(u) = n.as_u64() {
        return (u <= max_safe_u64).then_some(u as f64);
    }
    if let Some(f) = n.as_f64() {
        return (f.is_finite() && f.abs() <= WEBHOOK_MAX_SAFE_INTEGER as f64).then_some(f);
    }
    None
}

pub fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(if *b { "true" } else { "false" }.to_string()),
        Value::Number(n) => {
            let f = is_exact_json_number(n)?;
            if f == 0.0 {
                Some("0".to_string())
            } else if f.fract() == 0.0 {
                Some(format!("{}", f as i64))
            } else {
                Some(f.to_string())
            }
        }
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn is_strict_decimal_number(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[i] == b'-' {
        i += 1;
        if i >= bytes.len() {
            return false;
        }
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let has_int = i > int_start;
    let mut has_frac = false;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        has_frac = i > frac_start;
    }
    if !has_int && !has_frac {
        return false;
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        i += 1;
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == bytes.len()
}

/// Exact scalar equality matching JS webhook-matcher semantics (safe-integer precision,
/// numeric string comparison for numbers, case-sensitive exact comparison for strings/bools).
pub fn equals_value(item: &Value, expected: &str) -> bool {
    if let Value::Number(n) = item {
        let Some(item_f) = is_exact_json_number(n) else {
            return false;
        };
        if !is_strict_decimal_number(expected) {
            return false;
        }
        let Ok(exp_f) = expected.parse::<f64>() else {
            return false;
        };
        return exp_f.is_finite()
            && exp_f.abs() <= WEBHOOK_MAX_SAFE_INTEGER as f64
            && item_f == exp_f;
    }
    scalar_text(item).as_deref() == Some(expected)
}

/// Iterative (non-recursive) budget guard: depth `<= 128` containers and `<= 100_000` nodes.
pub fn webhook_body_fits(body: &Value) -> bool {
    let mut nodes: usize = 1;
    let mut stack: Vec<(&Value, usize)> = Vec::new();
    if matches!(body, Value::Array(_) | Value::Object(_)) {
        stack.push((body, 1));
    }
    while let Some((val, depth)) = stack.pop() {
        if depth > WEBHOOK_BODY_MAX_DEPTH {
            return false;
        }
        match val {
            Value::Array(arr) => {
                nodes = nodes.saturating_add(arr.len());
                if nodes > WEBHOOK_BODY_MAX_NODES {
                    return false;
                }
                for child in arr {
                    if matches!(child, Value::Array(_) | Value::Object(_)) {
                        stack.push((child, depth.saturating_add(1)));
                    }
                }
            }
            Value::Object(map) => {
                nodes = nodes.saturating_add(map.len());
                if nodes > WEBHOOK_BODY_MAX_NODES {
                    return false;
                }
                for child in map.values() {
                    if matches!(child, Value::Array(_) | Value::Object(_)) {
                        stack.push((child, depth.saturating_add(1)));
                    }
                }
            }
            _ => {}
        }
    }
    true
}

/// Extracts folding subject from `subject_from` path (returns `None` for non-scalar or unsafe int).
pub fn webhook_call_subject(subject_from: Option<&str>, call: &WebhookCallView<'_>) -> Option<String> {
    let path = subject_from?;
    let steps = parse_webhook_path(path).ok()?;
    let raw = match resolve_webhook_path(&steps, call)? {
        ResolvedWebhookValue::Header(h) => h.to_string(),
        ResolvedWebhookValue::Json(v) => scalar_text(v)?,
        ResolvedWebhookValue::HeadersRoot => return None,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > WEBHOOK_MAX_SUBJECT_LENGTH {
        return None;
    }
    Some(trimmed.to_string())
}

fn clause_matches(clause: &WebhookWhereClause, call: &WebhookCallView<'_>) -> bool {
    let Ok(steps) = parse_webhook_path(&clause.path) else {
        return false;
    };
    let resolved = resolve_webhook_path(&steps, call);
    let is_present = match resolved {
        None | Some(ResolvedWebhookValue::Json(Value::Null)) => false,
        Some(_) => true,
    };
    match clause.op {
        WebhookWhereOp::Exists => is_present,
        WebhookWhereOp::NotExists => !is_present,
        WebhookWhereOp::Eq => {
            let Some(expected) = clause.values.first() else {
                return false;
            };
            match resolved {
                Some(ResolvedWebhookValue::Header(h)) => h == expected,
                Some(ResolvedWebhookValue::Json(Value::Array(arr))) => {
                    arr.iter().any(|el| equals_value(el, expected))
                }
                Some(ResolvedWebhookValue::Json(v)) if !v.is_null() => equals_value(v, expected),
                _ => false,
            }
        }
        WebhookWhereOp::NotEq => {
            let Some(expected) = clause.values.first() else {
                return false;
            };
            let eq_match = match resolved {
                Some(ResolvedWebhookValue::Header(h)) => h == expected,
                Some(ResolvedWebhookValue::Json(Value::Array(arr))) => {
                    arr.iter().any(|el| equals_value(el, expected))
                }
                Some(ResolvedWebhookValue::Json(v)) if !v.is_null() => equals_value(v, expected),
                _ => false,
            };
            !eq_match
        }
        WebhookWhereOp::In => match resolved {
            Some(ResolvedWebhookValue::Header(h)) => clause.values.iter().any(|e| h == e),
            Some(ResolvedWebhookValue::Json(Value::Array(arr))) => arr
                .iter()
                .any(|el| clause.values.iter().any(|e| equals_value(el, e))),
            Some(ResolvedWebhookValue::Json(v)) if !v.is_null() => {
                clause.values.iter().any(|e| equals_value(v, e))
            }
            _ => false,
        },
        WebhookWhereOp::Contains => {
            let Some(needle) = clause.values.first() else {
                return false;
            };
            let needle_lc = needle.to_lowercase();
            match resolved {
                Some(ResolvedWebhookValue::Header(h)) => h.to_lowercase().contains(&needle_lc),
                Some(ResolvedWebhookValue::Json(Value::Array(arr))) => arr
                    .iter()
                    .any(|el| scalar_text(el).is_some_and(|s| s.to_lowercase().contains(&needle_lc))),
                Some(ResolvedWebhookValue::Json(v)) if !v.is_null() => {
                    scalar_text(v).is_some_and(|s| s.to_lowercase().contains(&needle_lc))
                }
                _ => false,
            }
        }
    }
}

/// Evaluates `filters` (`where` AND clauses, optional `text_contains`, optional `subject`)
/// against `call`. Deep/oversized bodies skip `text_contains` serialization and return `false`.
pub fn matches_webhook_call(filters: &WebhookFilters, call: &WebhookCallView<'_>) -> bool {
    if !filters
        .where_clauses
        .iter()
        .all(|c| clause_matches(c, call))
    {
        return false;
    }
    if let Some(ref needle) = filters.text_contains {
        let needle_lc = needle.to_lowercase();
        let matched = match call.body {
            Value::String(s) => s.to_lowercase().contains(&needle_lc),
            other => {
                if !webhook_body_fits(other) {
                    false
                } else {
                    serde_json::to_string(other)
                        .is_ok_and(|json| json.to_lowercase().contains(&needle_lc))
                }
            }
        };
        if !matched {
            return false;
        }
    }
    if let Some(ref expected_subject) = filters.subject {
        let Some(actual) = webhook_call_subject(filters.subject_from.as_deref(), call) else {
            return false;
        };
        if !actual.eq_ignore_ascii_case(expected_subject)
            && actual.to_lowercase() != expected_subject.to_lowercase()
        {
            return false;
        }
    }
    true
}

pub fn validate_webhook_filters(filters: &WebhookFilters) -> Result<(), BotError> {
    if filters.where_clauses.len() > WEBHOOK_MAX_WHERE {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Webhook filters cannot have more than {WEBHOOK_MAX_WHERE} `where` clauses."),
        ));
    }
    if filters.pick.len() > WEBHOOK_MAX_PICK {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Webhook filters cannot have more than {WEBHOOK_MAX_PICK} `pick` paths."),
        ));
    }
    for clause in &filters.where_clauses {
        parse_webhook_path(&clause.path)?;
        let arity_ok = match clause.op {
            WebhookWhereOp::Eq | WebhookWhereOp::NotEq | WebhookWhereOp::Contains => {
                clause.values.len() == 1
            }
            WebhookWhereOp::In => {
                !clause.values.is_empty() && clause.values.len() <= WEBHOOK_MAX_VALUES
            }
            WebhookWhereOp::Exists | WebhookWhereOp::NotExists => clause.values.is_empty(),
        };
        if !arity_ok {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Webhook `where` op '{}' received invalid number of values ({}).",
                    clause.op.as_str(),
                    clause.values.len()
                ),
            ));
        }
        for val in &clause.values {
            if val.is_empty() || val.chars().count() > WEBHOOK_MAX_VALUE_LENGTH {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Webhook filter value must be 1..={WEBHOOK_MAX_VALUE_LENGTH} characters."
                    ),
                ));
            }
        }
    }
    for p in &filters.pick {
        parse_webhook_path(p)?;
    }
    if let Some(ref sf) = filters.subject_from {
        parse_webhook_path(sf)?;
    }
    if let Some(ref tc) = filters.text_contains
        && (tc.is_empty() || tc.chars().count() > WEBHOOK_MAX_VALUE_LENGTH)
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Webhook `text_contains` must be 1..={WEBHOOK_MAX_VALUE_LENGTH} characters."),
        ));
    }
    if let Some(ref subj) = filters.subject {
        if filters.subject_from.is_none() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Webhook `subject` filter requires `subject_from` to be set.",
            ));
        }
        if subj.is_empty() || subj.chars().count() > WEBHOOK_MAX_VALUE_LENGTH {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Webhook `subject` filter must be 1..={WEBHOOK_MAX_VALUE_LENGTH} characters."),
            ));
        }
    }
    Ok(())
}

/// Validates serialized UTF-8 byte length of a webhook rule against `WEBHOOK_RULE_MAX_BYTES` (`2_560`).
pub fn validate_webhook_rule_bytes(rule: &WebhookRuleWire) -> Result<usize, BotError> {
    if rule.source != AutomationEventSource::Webhook {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "Webhook rule requires source 'webhook' (got '{}').",
                rule.source.as_str()
            ),
        ));
    }
    if rule.automation_id.trim().is_empty() || rule.bot_id.trim().is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Webhook rule requires non-empty automation_id and bot_id.",
        ));
    }
    if hook_id_of_rule(&rule.id).is_none() || rule.rev == 0 {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Webhook rule requires valid `wh-wh_<16>` id and rev >= 1.",
        ));
    }
    validate_webhook_filters(&rule.filters)?;
    let bytes = serde_json::to_vec(rule).map_err(|e| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Failed to serialize webhook rule: {e}"),
        )
    })?;
    if bytes.len() > WEBHOOK_RULE_MAX_BYTES {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "Serialized webhook rule ({} UTF-8 bytes) exceeds limit of {WEBHOOK_RULE_MAX_BYTES} bytes.",
                bytes.len()
            ),
        ));
    }
    Ok(bytes.len())
}

// ---------------------------------------------------------------------------
// 14 WebhookCallState Variants, 9 AutomationRunStatus Variants & Transitions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookCallState {
    Queued,
    Ignored,
    Paused,
    Delivered,
    Cancelled,
    WaitingCapacity,
    Started,
    Folded,
    Refused,
    Missed,
    Unavailable,
    Completed,
    Failed,
    Unverifiable,
}

impl WebhookCallState {
    pub const ALL: [Self; 14] = [
        Self::Queued,
        Self::Ignored,
        Self::Paused,
        Self::Delivered,
        Self::Cancelled,
        Self::WaitingCapacity,
        Self::Started,
        Self::Folded,
        Self::Refused,
        Self::Missed,
        Self::Unavailable,
        Self::Completed,
        Self::Failed,
        Self::Unverifiable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Ignored => "ignored",
            Self::Paused => "paused",
            Self::Delivered => "delivered",
            Self::Cancelled => "cancelled",
            Self::WaitingCapacity => "waiting_capacity",
            Self::Started => "started",
            Self::Folded => "folded",
            Self::Refused => "refused",
            Self::Missed => "missed",
            Self::Unavailable => "unavailable",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unverifiable => "unverifiable",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// Returns `true` for the 7 settled host-execution terminal states.
    pub const fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Refused
                | Self::Missed
                | Self::Unavailable
                | Self::Completed
                | Self::Failed
                | Self::Unverifiable
        )
    }

    /// Returns `true` for all 10 irreversible terminal/disposition states.
    pub const fn is_terminal(self) -> bool {
        self.is_settled() || matches!(self, Self::Ignored | Self::Paused | Self::Folded)
    }
}

/// Validates webhook call state machine transitions. All terminal states are irreversible.
pub const fn can_transition_webhook_call_state(
    from: WebhookCallState,
    to: WebhookCallState,
) -> bool {
    if from.is_terminal() {
        return false;
    }
    match from {
        WebhookCallState::Queued => matches!(
            to,
            WebhookCallState::Delivered
                | WebhookCallState::WaitingCapacity
                | WebhookCallState::Folded
                | WebhookCallState::Cancelled
                | WebhookCallState::Refused
                | WebhookCallState::Missed
                | WebhookCallState::Unavailable
                | WebhookCallState::Ignored
                | WebhookCallState::Paused
        ),
        WebhookCallState::Delivered => matches!(
            to,
            WebhookCallState::WaitingCapacity
                | WebhookCallState::Started
                | WebhookCallState::Folded
                | WebhookCallState::Cancelled
                | WebhookCallState::Refused
                | WebhookCallState::Missed
                | WebhookCallState::Unavailable
                | WebhookCallState::Unverifiable
        ),
        WebhookCallState::WaitingCapacity => matches!(
            to,
            WebhookCallState::Started
                | WebhookCallState::WaitingCapacity
                | WebhookCallState::Folded
                | WebhookCallState::Cancelled
                | WebhookCallState::Refused
                | WebhookCallState::Missed
                | WebhookCallState::Unavailable
                | WebhookCallState::Unverifiable
        ),
        WebhookCallState::Started => matches!(
            to,
            WebhookCallState::Completed
                | WebhookCallState::Failed
                | WebhookCallState::Missed
                | WebhookCallState::Unavailable
                | WebhookCallState::Unverifiable
                | WebhookCallState::Cancelled
        ),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    Pending,
    Dispatching,
    Dispatched,
    Completed,
    SkippedPrecheck,
    SkippedMissed,
    SkippedUnavailable,
    SkippedNeedsInteractiveAuth,
    DispatchFailed,
}

impl AutomationRunStatus {
    pub const ALL: [Self; 9] = [
        Self::Pending,
        Self::Dispatching,
        Self::Dispatched,
        Self::Completed,
        Self::SkippedPrecheck,
        Self::SkippedMissed,
        Self::SkippedUnavailable,
        Self::SkippedNeedsInteractiveAuth,
        Self::DispatchFailed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatching => "dispatching",
            Self::Dispatched => "dispatched",
            Self::Completed => "completed",
            Self::SkippedPrecheck => "skipped_precheck",
            Self::SkippedMissed => "skipped_missed",
            Self::SkippedUnavailable => "skipped_unavailable",
            Self::SkippedNeedsInteractiveAuth => "skipped_needs_interactive_auth",
            Self::DispatchFailed => "dispatch_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunTrigger {
    Scheduled,
    Manual,
    Event,
}

pub const fn is_final_automation_run_status(status: AutomationRunStatus) -> bool {
    matches!(
        status,
        AutomationRunStatus::Completed
            | AutomationRunStatus::DispatchFailed
            | AutomationRunStatus::SkippedPrecheck
            | AutomationRunStatus::SkippedMissed
            | AutomationRunStatus::SkippedUnavailable
            | AutomationRunStatus::SkippedNeedsInteractiveAuth
    )
}

pub fn claims_the_run(status: AutomationRunStatus) -> bool {
    !status.as_str().starts_with("skipped_")
}

pub const fn settled_run_outcome(status: AutomationRunStatus) -> Option<WebhookCallState> {
    match status {
        AutomationRunStatus::Completed => Some(WebhookCallState::Completed),
        AutomationRunStatus::DispatchFailed => Some(WebhookCallState::Failed),
        AutomationRunStatus::SkippedMissed => Some(WebhookCallState::Missed),
        AutomationRunStatus::SkippedUnavailable
        | AutomationRunStatus::SkippedPrecheck
        | AutomationRunStatus::SkippedNeedsInteractiveAuth => Some(WebhookCallState::Unavailable),
        AutomationRunStatus::Pending
        | AutomationRunStatus::Dispatching
        | AutomationRunStatus::Dispatched => None,
    }
}

// ---------------------------------------------------------------------------
// Rolling-Hour Cap Tracker, Epoch Fencing & Zero-Leak WebhookTriggerEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookNoticeCondition {
    CapReached,
    RefusalBurst,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookNotice {
    pub kind: String,
    pub hook_id: String,
    pub bot_id: String,
    pub condition: WebhookNoticeCondition,
    pub count: u32,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookCapDecision {
    Allowed {
        hook_used: u32,
        hook_cap: u32,
        account_used: u32,
        account_cap: u32,
    },
    Capped {
        retry_at_ms: u64,
        hook_capped: bool,
        account_capped: bool,
        notice: Option<WebhookNotice>,
    },
}

/// Pure in-memory rolling-hour (`3_600_000` ms) webhook cap tracker enforcing both
/// per-hook cap (default `200`, configurable `1..=200`) and account cap (`300`),
/// plus de-spammed `cap_reached` and `refusal_burst` notices.
#[derive(Debug, Clone, Default)]
pub struct WebhookHourlyCapTracker {
    account_starts: VecDeque<u64>,
    hook_starts: BTreeMap<String, VecDeque<u64>>,
    hook_custom_caps: BTreeMap<String, u32>,
    hook_refusals: BTreeMap<String, VecDeque<u64>>,
    last_cap_notice_at: BTreeMap<String, u64>,
    last_refusal_burst_notice_at: BTreeMap<String, u64>,
}

impl WebhookHourlyCapTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_hook_cap(&mut self, hook_id: &str, cap: u32) -> Result<(), BotError> {
        if cap == 0 || cap > WEBHOOK_HOOK_HOURLY_CAP {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Custom webhook hourly cap must be in 1..={WEBHOOK_HOOK_HOURLY_CAP}."),
            ));
        }
        self.hook_custom_caps.insert(hook_id.to_string(), cap);
        Ok(())
    }

    pub fn effective_hook_cap(&self, hook_id: &str) -> u32 {
        self.hook_custom_caps
            .get(hook_id)
            .copied()
            .unwrap_or(WEBHOOK_HOOK_HOURLY_CAP)
    }

    fn evict_expired(queue: &mut VecDeque<u64>, now_ms: u64, window_ms: u64) {
        queue.retain(|&ts| now_ms.saturating_sub(ts) < window_ms);
    }

    fn retry_at_for_cap(queue: &VecDeque<u64>, cap: u32, now_ms: u64) -> u64 {
        let cap_usize = cap as usize;
        if queue.is_empty() || queue.len() < cap_usize {
            return 0;
        }
        let mut sorted: Vec<u64> = queue.iter().copied().collect();
        sorted.sort_unstable();
        let idx = if cap_usize == 0 {
            sorted.len().saturating_sub(1)
        } else {
            sorted.len().saturating_sub(cap_usize)
        };
        sorted
            .get(idx)
            .copied()
            .unwrap_or(now_ms)
            .saturating_add(WEBHOOK_CAP_WINDOW_MS)
    }

    fn can_emit_notice(last_map: &BTreeMap<String, u64>, hook_id: &str, now_ms: u64) -> bool {
        match last_map.get(hook_id) {
            None => true,
            Some(&prev) => now_ms.saturating_sub(prev) >= WEBHOOK_NOTICE_COOLDOWN_MS,
        }
    }

    pub fn try_admit_start(
        &mut self,
        hook_id: &str,
        bot_id: &str,
        now_ms: u64,
    ) -> WebhookCapDecision {
        Self::evict_expired(&mut self.account_starts, now_ms, WEBHOOK_CAP_WINDOW_MS);
        let hook_cap = self.effective_hook_cap(hook_id);
        let hook_queue = self.hook_starts.entry(hook_id.to_string()).or_default();
        Self::evict_expired(hook_queue, now_ms, WEBHOOK_CAP_WINDOW_MS);

        let hook_used = hook_queue.len() as u32;
        let account_used = self.account_starts.len() as u32;
        let hook_capped = hook_used >= hook_cap;
        let account_capped = account_used >= WEBHOOK_ACCOUNT_HOURLY_CAP;

        if !hook_capped && !account_capped {
            hook_queue.push_back(now_ms);
            self.account_starts.push_back(now_ms);
            return WebhookCapDecision::Allowed {
                hook_used: hook_used.saturating_add(1),
                hook_cap,
                account_used: account_used.saturating_add(1),
                account_cap: WEBHOOK_ACCOUNT_HOURLY_CAP,
            };
        }

        let hook_retry = if hook_capped {
            Self::retry_at_for_cap(hook_queue, hook_cap, now_ms)
        } else {
            0
        };
        let account_retry = if account_capped {
            Self::retry_at_for_cap(&self.account_starts, WEBHOOK_ACCOUNT_HOURLY_CAP, now_ms)
        } else {
            0
        };
        let retry_at_ms = hook_retry.max(account_retry);

        let notice = if Self::can_emit_notice(&self.last_cap_notice_at, hook_id, now_ms) {
            self.last_cap_notice_at.insert(hook_id.to_string(), now_ms);
            let limit = if hook_capped {
                hook_cap
            } else {
                WEBHOOK_ACCOUNT_HOURLY_CAP
            };
            Some(WebhookNotice {
                kind: "webhook_notice".to_string(),
                hook_id: hook_id.to_string(),
                bot_id: bot_id.to_string(),
                condition: WebhookNoticeCondition::CapReached,
                count: limit,
                at: now_ms,
            })
        } else {
            None
        };

        WebhookCapDecision::Capped {
            retry_at_ms,
            hook_capped,
            account_capped,
            notice,
        }
    }

    pub fn record_refusal(
        &mut self,
        hook_id: &str,
        bot_id: &str,
        now_ms: u64,
    ) -> Option<WebhookNotice> {
        let queue = self.hook_refusals.entry(hook_id.to_string()).or_default();
        Self::evict_expired(queue, now_ms, WEBHOOK_REFUSAL_BURST_WINDOW_MS);
        queue.push_back(now_ms);
        let count = queue.len() as u32;
        if count >= WEBHOOK_REFUSAL_BURST_THRESHOLD
            && Self::can_emit_notice(&self.last_refusal_burst_notice_at, hook_id, now_ms)
        {
            self.last_refusal_burst_notice_at
                .insert(hook_id.to_string(), now_ms);
            return Some(WebhookNotice {
                kind: "webhook_notice".to_string(),
                hook_id: hook_id.to_string(),
                bot_id: bot_id.to_string(),
                condition: WebhookNoticeCondition::RefusalBurst,
                count,
                at: now_ms,
            });
        }
        None
    }
}

/// Epoch fencing check: rejects stale deliveries where `delivery_epoch < current_epoch`.
pub fn check_webhook_epoch(delivery_epoch: u64, current_epoch: u64) -> Result<(), BotError> {
    if delivery_epoch < current_epoch {
        return Err(BotError::new(
            BotErrorCode::Expired,
            format!("Stale webhook delivery epoch {delivery_epoch} (current epoch is {current_epoch})."),
        ));
    }
    Ok(())
}

pub const fn advance_webhook_epoch(current_epoch: u64) -> u64 {
    current_epoch.saturating_add(1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WebhookCallStartAnswer {
    Started {
        #[serde(default, alias = "runId", skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    WaitingCapacity {
        #[serde(alias = "retryAt")]
        retry_at: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum WebhookCallStartError {
    StartedForAnotherRun {
        #[serde(alias = "runId")]
        run_id: String,
    },
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookCallFields {
    pub call_id: String,
    pub epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_gen: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<bool>,
}

/// Zero-leak webhook trigger frame.
/// STRICT INVARIANT: NEVER carries `text`, `url`, `actor`, `place`, `fromOwner`,
/// `onOwnerMessage`, or `pr` fields; `deny_unknown_fields` rejects them on decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookTriggerEvent {
    pub key: String,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub occurred_at: u64,
    pub webhook: WebhookCallFields,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<WebhookAdapterId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule_id: Option<String>,
}

impl WebhookTriggerEvent {
    pub fn validate(&self) -> Result<(), BotError> {
        if !is_valid_webhook_call_id(&self.key) || self.key != self.webhook.call_id {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "WebhookTriggerEvent `key` must be a valid call id equal to `webhook.call_id`.",
            ));
        }
        let Some(hook_id) = hook_id_of_actor(&self.actor_id) else {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "WebhookTriggerEvent `actor_id` must match `webhook:wh_<16>`.",
            ));
        };
        if let Some(ref rule_id) = self.matched_rule_id
            && hook_id_of_rule(rule_id) != Some(hook_id)
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "WebhookTriggerEvent `matched_rule_id` does not match `actor_id` hook id.",
            ));
        }
        if let Some(ref subj) = self.subject {
            if subj.trim().is_empty() || subj.chars().count() > WEBHOOK_MAX_SUBJECT_LENGTH {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "WebhookTriggerEvent `subject` must be 1..={WEBHOOK_MAX_SUBJECT_LENGTH} characters."
                    ),
                ));
            }
            if redact_webhook_keys(subj) != *subj {
                return Err(BotError::new(
                    BotErrorCode::SecretBlocked,
                    "WebhookTriggerEvent `subject` must not contain webhook secret keys.",
                ));
            }
        }
        if self.webhook.sample == Some(false) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "WebhookCallFields `sample` may only be omitted or `true`.",
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OneShotWait & WatchPrPolicy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneShotRunSummary {
    pub trigger: AutomationRunTrigger,
    pub status: AutomationRunStatus,
}

/// Returns `true` iff any non-manual run has claimed the one-shot wait
/// (all `skipped_*` statuses and `Manual` runs never consume the wait).
pub fn once_claimed(runs: &[OneShotRunSummary]) -> bool {
    runs.iter()
        .any(|r| r.trigger != AutomationRunTrigger::Manual && claims_the_run(r.status))
}

/// Returns `true` iff at least one non-manual run claimed the wait and all
/// claiming runs have reached a final status.
pub fn once_run_finished(runs: &[OneShotRunSummary]) -> bool {
    let mut found_claim = false;
    for r in runs {
        if r.trigger != AutomationRunTrigger::Manual && claims_the_run(r.status) {
            found_claim = true;
            if !is_final_automation_run_status(r.status) {
                return false;
            }
        }
    }
    found_claim
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneShotWait {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<u64>,
}

impl OneShotWait {
    pub const fn due_at(self, created_at_ms: u64) -> u64 {
        match self.by {
            Some(by) => by,
            None => created_at_ms.saturating_add(WAIT_EXPIRY_MS),
        }
    }

    pub fn validate(
        self,
        created_at_ms: u64,
        has_bot: bool,
        has_schedule: bool,
        has_events: bool,
    ) -> Result<(), BotError> {
        if !has_bot {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Only bot automations can run once.",
            ));
        }
        if has_schedule {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "A one-shot wait cannot have a recurring schedule.",
            ));
        }
        if !has_events && self.by.is_none() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "A one-shot wait requires at least one event trigger or a deadline (`by`).",
            ));
        }
        if let Some(by) = self.by {
            if by <= created_at_ms {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "One-shot wait deadline (`by`) must be strictly after creation time.",
                ));
            }
            if by.saturating_sub(created_at_ms) > WAIT_MAX_DEADLINE_MS {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "One-shot wait deadline exceeds maximum horizon of {WAIT_MAX_DEADLINE_MS} ms."
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitEnding {
    Event {
        at: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
    },
    Deadline {
        at: u64,
    },
    Expired {
        at: u64,
    },
}

pub fn once_run_ending(
    once_present: bool,
    trigger: AutomationRunTrigger,
    scheduled_for: Option<u64>,
    event_occurred_at: Option<u64>,
    event_actor: Option<String>,
) -> Option<WaitEnding> {
    if !once_present {
        return None;
    }
    match trigger {
        AutomationRunTrigger::Scheduled => scheduled_for.map(|at| WaitEnding::Deadline { at }),
        AutomationRunTrigger::Event => event_occurred_at.map(|at| WaitEnding::Event {
            at,
            actor: event_actor,
        }),
        AutomationRunTrigger::Manual => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWaitEntry {
    pub id: String,
    pub wait: OneShotWait,
    pub runs: Vec<OneShotRunSummary>,
}

/// Filters unclaimed waits and sorts ascending by `wait.by.unwrap_or(u64::MAX)`
/// (soonest deadlines first; no-deadline waits last).
pub fn pending_waits(entries: &[PendingWaitEntry]) -> Vec<PendingWaitEntry> {
    let mut pending: Vec<PendingWaitEntry> = entries
        .iter()
        .filter(|e| !once_claimed(&e.runs))
        .cloned()
        .collect();
    pending.sort_by_key(|e| e.wait.by.unwrap_or(u64::MAX));
    pending
}

/// Returns the GitHub events watched by `watch-pr`:
/// 7 events when `ci_finished` is supported, 8 events (`ci_passed` + `ci_failed`) otherwise.
pub fn watch_pr_events(ci_finished_supported: bool) -> Vec<GitHubEventKind> {
    if ci_finished_supported {
        vec![
            GitHubEventKind::CiFinished,
            GitHubEventKind::PrConflicted,
            GitHubEventKind::PrMerged,
            GitHubEventKind::PrClosed,
            GitHubEventKind::Review,
            GitHubEventKind::ReviewComment,
            GitHubEventKind::Comment,
        ]
    } else {
        vec![
            GitHubEventKind::CiPassed,
            GitHubEventKind::CiFailed,
            GitHubEventKind::PrConflicted,
            GitHubEventKind::PrMerged,
            GitHubEventKind::PrClosed,
            GitHubEventKind::Review,
            GitHubEventKind::ReviewComment,
            GitHubEventKind::Comment,
        ]
    }
}

fn is_valid_github_slug_part(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn parse_positive_pr_number(raw: &str) -> Result<u64, BotError> {
    let bytes = raw.as_bytes();
    if bytes.is_empty()
        || !matches!(bytes[0], b'1'..=b'9')
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid PR number '{raw}'."),
        ));
    }
    raw.parse::<u64>().map_err(|_| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("PR number '{raw}' overflowed."),
        )
    })
}

/// Parses `owner/repo#123` or `https?://(www.)?github.com/owner/repo/pull/123[/(files|checks|commits)][/][?#...]`
/// into canonical `"owner/repo#123"`.
pub fn parse_watched_pr(raw: &str) -> Result<String, BotError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Watched PR reference cannot be empty.",
        ));
    }
    if let Some((scheme, after_scheme)) = trimmed.split_once("://") {
        if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Watched PR URL must use http or https.",
            ));
        }
        let no_frag = after_scheme.split_once('#').map_or(after_scheme, |(b, _)| b);
        let no_query = no_frag.split_once('?').map_or(no_frag, |(b, _)| b);
        let (host, path) = no_query.split_once('/').ok_or_else(|| {
            BotError::new(BotErrorCode::InvalidArgument, "Invalid GitHub PR URL.")
        })?;
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Watched PR URL must point to github.com.",
            ));
        }
        let path_trimmed = path.trim_end_matches('/');
        let segs: Vec<&str> = path_trimmed.split('/').collect();
        if !(segs.len() == 4
            || (segs.len() == 5
                && (segs[4].eq_ignore_ascii_case("files")
                    || segs[4].eq_ignore_ascii_case("checks")
                    || segs[4].eq_ignore_ascii_case("commits"))))
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "GitHub PR URL must match /owner/repo/pull/<number>.",
            ));
        }
        if !segs[2].eq_ignore_ascii_case("pull")
            || !is_valid_github_slug_part(segs[0])
            || !is_valid_github_slug_part(segs[1])
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Invalid owner/repo/pull path in GitHub PR URL.",
            ));
        }
        let num = parse_positive_pr_number(segs[3])?;
        return Ok(format!("{}/{}#{num}", segs[0], segs[1]));
    }

    let (repo_part, num_part) = trimmed.split_once('#').ok_or_else(|| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            "Watched PR must be `owner/repo#<number>` or a GitHub PR URL.",
        )
    })?;
    let (owner, repo) = repo_part.split_once('/').ok_or_else(|| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            "Watched PR must include `owner/repo`.",
        )
    })?;
    if repo.contains('/') || !is_valid_github_slug_part(owner) || !is_valid_github_slug_part(repo) {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Invalid `owner/repo` slug in watched PR.",
        ));
    }
    let num = parse_positive_pr_number(num_part)?;
    Ok(format!("{owner}/{repo}#{num}"))
}

/// Parses `<H>h`, `<M>m`, or `<H>h<M>m` (case-insensitive) into milliseconds with
/// strict overflow safety and upper bound `WAIT_MAX_DEADLINE_MS` (`31_104_000_000` ms).
pub fn parse_watch_duration(raw: &str) -> Result<u64, BotError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Watch duration cannot be empty.",
        ));
    }
    let bytes = trimmed.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut hours: Option<u64> = None;
    let mut minutes: Option<u64> = None;

    let read_digits = |start: usize| -> Result<(u64, usize), BotError> {
        let mut p = start;
        while p < len && bytes[p].is_ascii_digit() {
            p += 1;
        }
        if p == start {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Invalid watch duration '{trimmed}'."),
            ));
        }
        let num_str = trimmed.get(start..p).ok_or_else(|| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Invalid watch duration '{trimmed}'."),
            )
        })?;
        let val = num_str.parse::<u64>().map_err(|_| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Watch duration '{trimmed}' overflowed."),
            )
        })?;
        Ok((val, p))
    };

    let (first_val, next_i) = read_digits(i)?;
    let unit1 = *bytes.get(next_i).ok_or_else(|| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Watch duration '{trimmed}' is missing unit ('h' or 'm')."),
        )
    })?;
    i = next_i + 1;

    match unit1 {
        b'h' | b'H' => {
            hours = Some(first_val);
            if i < len {
                let (min_val, after_min) = read_digits(i)?;
                let unit2 = *bytes.get(after_min).ok_or_else(|| {
                    BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Watch duration '{trimmed}' is missing 'm' unit."),
                    )
                })?;
                if !matches!(unit2, b'm' | b'M') || after_min + 1 != len {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Invalid watch duration '{trimmed}'."),
                    ));
                }
                minutes = Some(min_val);
            }
        }
        b'm' | b'M' => {
            if i != len {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!("Invalid watch duration '{trimmed}'."),
                ));
            }
            minutes = Some(first_val);
        }
        _ => {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Invalid watch duration unit in '{trimmed}'."),
            ));
        }
    }

    let h_ms = hours
        .unwrap_or(0)
        .checked_mul(3_600_000)
        .ok_or_else(|| BotError::new(BotErrorCode::InvalidArgument, "Watch duration overflowed."))?;
    let m_ms = minutes
        .unwrap_or(0)
        .checked_mul(60_000)
        .ok_or_else(|| BotError::new(BotErrorCode::InvalidArgument, "Watch duration overflowed."))?;
    let total_ms = h_ms
        .checked_add(m_ms)
        .ok_or_else(|| BotError::new(BotErrorCode::InvalidArgument, "Watch duration overflowed."))?;

    if total_ms == 0 || total_ms > WAIT_MAX_DEADLINE_MS {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "Watch duration must be > 0 and <= {WAIT_MAX_DEADLINE_MS} ms (got {total_ms} ms)."
            ),
        ));
    }
    Ok(total_ms)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPrPolicy {
    pub pr: String,
    pub deadline_ms: u64,
    pub max_fix_attempts: u8,
    pub ci_finished_supported: bool,
}

impl WatchPrPolicy {
    pub fn new(
        pr_raw: &str,
        now_ms: u64,
        duration_ms: Option<u64>,
        max_fix_attempts: Option<u8>,
        ci_finished_supported: bool,
    ) -> Result<Self, BotError> {
        let pr = parse_watched_pr(pr_raw)?;
        let dur = duration_ms.unwrap_or(WATCH_PR_DEFAULT_MS);
        if dur == 0 || dur > WAIT_MAX_DEADLINE_MS {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Watch-PR duration must be in 1..=WAIT_MAX_DEADLINE_MS.",
            ));
        }
        let attempts = max_fix_attempts.unwrap_or(WATCH_PR_MAX_FIX_ATTEMPTS);
        if attempts == 0 || attempts > WATCH_PR_MAX_FIX_ATTEMPTS {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Watch-PR max_fix_attempts must be in 1..={WATCH_PR_MAX_FIX_ATTEMPTS}."),
            ));
        }
        let deadline_ms = now_ms.saturating_add(dur);
        Ok(Self {
            pr,
            deadline_ms,
            max_fix_attempts: attempts,
            ci_finished_supported,
        })
    }

    pub fn events(&self) -> Vec<GitHubEventKind> {
        watch_pr_events(self.ci_finished_supported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchPrWakeOutcome {
    MergedOrClosed,
    ReadyToMerge,
    NeedsChange { authorized: bool },
    StillRunningOrQuestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchPrStopReason {
    MergedOrClosed,
    ReadyToMerge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchPrStepAction {
    AnnounceAndStop {
        reason: WatchPrStopReason,
    },
    NotifyOwnerAndRearmSameAttempt {
        attempt: u8,
    },
    DispatchWorkerFix {
        attempt: u8,
        next_attempt: u8,
        rearm_after_worker_push: bool,
    },
    EscalateToOwnerAndStop {
        attempt: u8,
    },
    ForwardQuestionAndRearmSameAttempt {
        attempt: u8,
    },
}

/// Evaluates the next watch-PR step, strictly enforcing the 2-attempt worker-fix ceiling
/// and refusing to dispatch workers on unauthorized review comments.
pub const fn evaluate_watch_pr_step(
    attempt: u8,
    outcome: WatchPrWakeOutcome,
    max_attempts: u8,
) -> WatchPrStepAction {
    let cap = if max_attempts > WATCH_PR_MAX_FIX_ATTEMPTS {
        WATCH_PR_MAX_FIX_ATTEMPTS
    } else {
        max_attempts
    };
    match outcome {
        WatchPrWakeOutcome::MergedOrClosed => WatchPrStepAction::AnnounceAndStop {
            reason: WatchPrStopReason::MergedOrClosed,
        },
        WatchPrWakeOutcome::ReadyToMerge => WatchPrStepAction::AnnounceAndStop {
            reason: WatchPrStopReason::ReadyToMerge,
        },
        WatchPrWakeOutcome::NeedsChange { authorized: false } => {
            WatchPrStepAction::NotifyOwnerAndRearmSameAttempt { attempt }
        }
        WatchPrWakeOutcome::NeedsChange { authorized: true } => {
            if attempt >= 1 && attempt <= cap {
                WatchPrStepAction::DispatchWorkerFix {
                    attempt,
                    next_attempt: attempt.saturating_add(1),
                    rearm_after_worker_push: true,
                }
            } else {
                WatchPrStepAction::EscalateToOwnerAndStop { attempt }
            }
        }
        WatchPrWakeOutcome::StillRunningOrQuestion => {
            WatchPrStepAction::ForwardQuestionAndRearmSameAttempt { attempt }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_bot_target(
        runs_in: AutomationBotRunsIn,
        results_to: AutomationBotResultsTo,
    ) -> AutomationBotTarget {
        AutomationBotTarget {
            bot_id: "bot-1".to_string(),
            runs_in,
            results_to,
            agent_setup: None,
            let_bot_react: None,
            precheck: normalize_automation_precheck("  echo ok  ", None),
            setup_trust_hash: None,
            missed_run_grace_minutes: None,
        }
    }

    fn base_connector_trigger(id: &str, kind: AutomationEventKind) -> AutomationConnectorEventTrigger {
        AutomationConnectorEventTrigger {
            id: id.to_string(),
            kind,
            in_places: Vec::new(),
            from: Vec::new(),
            text_contains: None,
            subject: None,
            label: None,
            run: None,
            on_owner_message: None,
            from_owner: None,
            names: BTreeMap::new(),
        }
    }

    #[test]
    fn tw_01_all_31_event_triggers_and_source_ownership() {
        assert_eq!(AutomationEventKind::ALL.len(), 31);
        let mut host_local = 0usize;
        for kind in AutomationEventKind::ALL {
            assert_eq!(
                AutomationEventKind::parse(kind.source().as_str(), kind.event_str()),
                Some(kind)
            );
            if kind.is_host_local_only() {
                host_local += 1;
                assert!(matches!(
                    kind.source(),
                    AutomationEventSource::Gmail | AutomationEventSource::Calendar
                ));
            }
        }
        assert_eq!(host_local, 2);

        // Cross-source swaps strictly rejected
        let swaps = [
            ("slack", "pr_opened"),
            ("github", "reaction"),
            ("linear", "new_email"),
            ("gmail", "call"),
            ("calendar", "assigned"),
            ("webhook", "mentioned"),
        ];
        for (src, ev) in swaps {
            assert_eq!(AutomationEventKind::parse(src, ev), None);
        }
    }

    #[test]
    fn tw_02_from_owner_exclusivity_only_github_pr_opened() {
        let once = OneShotWait { by: Some(10_000) };
        for kind in AutomationEventKind::ALL {
            if !kind.source().is_connector_source() {
                assert!(!kind.takes_own_events());
                continue;
            }
            let mut t = base_connector_trigger("t1", kind);
            t.from_owner = Some(true);
            if kind.is_pr_state_event() {
                t.subject = Some("org/repo#1".to_string());
            }
            if kind == AutomationEventKind::Github(GitHubEventKind::WorkflowFinished) {
                t.run = Some("12345".to_string());
                t.in_places = vec!["github:1:2".to_string()];
            }
            let res = validate_connector_trigger(&t, Some(&once));
            if kind == AutomationEventKind::Github(GitHubEventKind::PrOpened) {
                assert!(res.is_ok(), "github.pr_opened must allow from_owner=true");
            } else {
                assert_eq!(
                    res.unwrap_err().code,
                    BotErrorCode::InvalidArgument,
                    "expected from_owner=true rejected on {}.{}",
                    kind.source().as_str(),
                    kind.event_str()
                );
            }
        }
    }

    #[test]
    fn tw_03_github_google_thread_and_count_invariants() {
        let bot_new_thread =
            sample_bot_target(AutomationBotRunsIn::New, AutomationBotResultsTo::Thread);
        let bot_new_chat =
            sample_bot_target(AutomationBotRunsIn::New, AutomationBotResultsTo::Chat);
        let bot_chat_thread = sample_bot_target(
            AutomationBotRunsIn::Chat {
                session_id: "s1".to_string(),
                title: None,
                home_if_gone: None,
                to_conversation: None,
            },
            AutomationBotResultsTo::Thread,
        );

        // (a) label only on github.labeled
        let mut lbl_bad = base_connector_trigger("t1", AutomationEventKind::Github(GitHubEventKind::PrOpened));
        lbl_bad.label = Some("bug".to_string());
        assert!(validate_connector_trigger(&lbl_bad, None).is_err());
        let mut lbl_ok = base_connector_trigger("t1", AutomationEventKind::Github(GitHubEventKind::Labeled));
        lbl_ok.label = Some("bug".to_string());
        assert!(validate_connector_trigger(&lbl_ok, None).is_ok());

        // (b) PR state events require subject and forbid from
        for ev in [GitHubEventKind::CiPassed, GitHubEventKind::CiFinished, GitHubEventKind::PrConflicted] {
            let mut t = base_connector_trigger("t1", AutomationEventKind::Github(ev));
            assert!(validate_connector_trigger(&t, None).is_err());
            t.subject = Some("org/repo#9".to_string());
            assert!(validate_connector_trigger(&t, None).is_ok());
            t.from = vec!["u1".to_string()];
            assert!(validate_connector_trigger(&t, None).is_err());
        }

        // (c) github.workflow_finished invariants
        let once_with_by = OneShotWait { by: Some(9_999) };
        let mut wf = base_connector_trigger("wf1", AutomationEventKind::Github(GitHubEventKind::WorkflowFinished));
        wf.run = Some("12345".to_string());
        wf.in_places = vec!["github:1:2".to_string()];
        assert!(validate_connector_trigger(&wf, None).is_err()); // missing once.by
        assert!(validate_connector_trigger(&wf, Some(&once_with_by)).is_ok());
        wf.run = Some("abc".to_string());
        assert!(validate_connector_trigger(&wf, Some(&once_with_by)).is_err());
        wf.run = Some("1".repeat(21));
        assert!(validate_connector_trigger(&wf, Some(&once_with_by)).is_err());
        wf.run = Some("12345".to_string());
        wf.in_places = vec!["org/repo".to_string()];
        assert!(validate_connector_trigger(&wf, Some(&once_with_by)).is_err());
        wf.in_places = vec!["github:1:2".to_string(), "github:1:3".to_string()];
        assert!(validate_connector_trigger(&wf, Some(&once_with_by)).is_err());
        wf.in_places = vec!["github:1:2".to_string()];
        wf.subject = Some("org/repo#1".to_string());
        assert!(validate_connector_trigger(&wf, Some(&once_with_by)).is_err());

        // (d) calendar minutes_before 0..=1440
        for ok_min in [0u32, 1440] {
            assert!(validate_google_trigger(&AutomationGoogleTrigger::Calendar {
                id: "cal1".to_string(),
                account: None,
                calendar: None,
                minutes_before: ok_min,
            }).is_ok());
        }
        assert!(validate_google_trigger(&AutomationGoogleTrigger::Calendar {
            id: "cal1".to_string(),
            account: None,
            calendar: None,
            minutes_before: 1441,
        }).is_err());

        // (e) results_to == Thread invariants
        let slack_t = AutomationEventTrigger::Connector(base_connector_trigger(
            "s1",
            AutomationEventKind::Slack(SlackEventKind::Mentioned),
        ));
        assert!(validate_automation_event_triggers(
            std::slice::from_ref(&slack_t),
            Some(&bot_new_thread),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_ok());
        // Fails with schedule, with runs_in=Chat, with webhook, with gmail, with calendar
        assert!(validate_automation_event_triggers(
            std::slice::from_ref(&slack_t),
            Some(&bot_new_thread),
            1_700_000_000_000,
            None
        ).is_err());
        assert!(validate_automation_event_triggers(
            std::slice::from_ref(&slack_t),
            Some(&bot_chat_thread),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_err());
        let wh_t = AutomationEventTrigger::Webhook(AutomationWebhookTrigger {
            id: "wh-wh_AbCdEfGh12345678".to_string(),
            rev: 1,
            filters: WebhookFilters::default(),
        });
        assert!(validate_automation_event_triggers(
            std::slice::from_ref(&wh_t),
            Some(&bot_new_thread),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_err());
        let gmail_t = AutomationEventTrigger::Google(AutomationGoogleTrigger::Gmail {
            id: "g1".to_string(),
            account: None,
            from: vec![],
            to: vec![],
            subject_contains: None,
            label: None,
        });
        assert!(validate_automation_event_triggers(
            &[gmail_t],
            Some(&bot_new_thread),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_err());

        // (f) 20 triggers ok, 21 err; 2 webhook triggers err; dup id err
        let twenty: Vec<AutomationEventTrigger> = (0..20)
            .map(|i| {
                AutomationEventTrigger::Connector(base_connector_trigger(
                    &format!("t{i}"),
                    AutomationEventKind::Slack(SlackEventKind::Mentioned),
                ))
            })
            .collect();
        assert!(validate_automation_event_triggers(
            &twenty,
            Some(&bot_new_chat),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_ok());
        let mut twenty_one = twenty;
        twenty_one.push(AutomationEventTrigger::Connector(base_connector_trigger(
            "t20",
            AutomationEventKind::Slack(SlackEventKind::Mentioned),
        )));
        assert!(validate_automation_event_triggers(
            &twenty_one,
            Some(&bot_new_chat),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_err());

        let wh_t2 = AutomationEventTrigger::Webhook(AutomationWebhookTrigger {
            id: "wh-wh_ZzYyXxWw87654321".to_string(),
            rev: 1,
            filters: WebhookFilters::default(),
        });
        assert!(validate_automation_event_triggers(
            &[wh_t, wh_t2],
            Some(&bot_new_chat),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            None
        ).is_err());
    }

    #[test]
    fn tw_04_json_path_security_and_resolution() {
        let long_path = format!("body.{}", "a".repeat(196));
        assert_eq!(long_path.len(), 201);
        let invalid = [
            "",
            "data.title",
            "body.__proto__.x",
            "body.a.prototype",
            "body.constructor",
            "body..a",
            "body.",
            ".body",
            "body.a b",
            "body[a]",
            "body[-1]",
            "body[]",
            "body[1000000]",
            "body[0]foo",
            long_path.as_str(),
        ];
        for p in invalid {
            assert!(parse_webhook_path(p).is_err(), "expected invalid path: {p}");
        }

        assert!(parse_webhook_path("body[0].id").is_ok());
        assert!(parse_webhook_path("body[0][1]").is_ok());
        assert!(parse_webhook_path("body[999999]").is_ok());
        let h_steps = parse_webhook_path("headers.Sentry-Hook-Resource").expect("header path");
        assert_eq!(h_steps[1], WebhookPathStep::Key("sentry-hook-resource".to_string()));

        let mut headers = BTreeMap::new();
        headers.insert("sentry-hook-resource".to_string(), "issue".to_string());
        let body = json!({"data": {"x": 1}, "tags": ["a", "b"]});
        let call = WebhookCallView { headers: &headers, body: &body };
        assert_eq!(
            resolve_webhook_path(&h_steps, &call),
            Some(ResolvedWebhookValue::Header("issue"))
        );
        // Own-property-only: toString, Array.length, string[0] => None
        assert_eq!(
            resolve_webhook_path(&parse_webhook_path("body.data.toString").unwrap(), &call),
            None
        );
        assert_eq!(
            resolve_webhook_path(&parse_webhook_path("body.tags.length").unwrap(), &call),
            None
        );
        let str_body = Value::String("hello".to_string());
        let str_call = WebhookCallView { headers: &headers, body: &str_body };
        assert_eq!(
            resolve_webhook_path(&parse_webhook_path("body.0").unwrap(), &str_call),
            None
        );
    }
}

#[cfg(test)]
mod webhook_matcher_and_cap_tests {
    use super::*;
    use serde_json::json;

    fn nested_arrays(depth: usize) -> Value {
        let mut cur = Value::Array(Vec::new());
        for _ in 1..depth {
            cur = Value::Array(vec![cur]);
        }
        cur
    }

    fn iterative_drop(mut val: Value) {
        while let Value::Array(mut arr) = val {
            if let Some(child) = arr.pop() {
                val = child;
            } else {
                break;
            }
        }
    }

    #[test]
    fn tw_05_deep_json_and_node_budget_iterative_guard() {
        let d128 = nested_arrays(128);
        assert!(webhook_body_fits(&d128));
        iterative_drop(d128);

        let d129 = nested_arrays(129);
        assert!(!webhook_body_fits(&d129));
        iterative_drop(d129);

        // 10,000-deep array: zero stack overflow in webhook_body_fits & matches_webhook_call!
        let d10k = nested_arrays(10_000);
        assert!(!webhook_body_fits(&d10k));
        let headers = BTreeMap::new();
        let call = WebhookCallView { headers: &headers, body: &d10k };
        let exists_f = WebhookFilters {
            where_clauses: vec![WebhookWhereClause {
                path: "body[0]".to_string(),
                op: WebhookWhereOp::Exists,
                values: vec![],
            }],
            ..Default::default()
        };
        assert!(matches_webhook_call(&exists_f, &call));
        let text_f = WebhookFilters {
            text_contains: Some("[".to_string()),
            ..Default::default()
        };
        assert!(!matches_webhook_call(&text_f, &call));
        iterative_drop(d10k);

        // Flat array node budget: 99_999 scalars (100_000 nodes) ok; 100_000 scalars (100_001) fails
        let ok_flat = Value::Array(vec![Value::from(0); 99_999]);
        assert!(webhook_body_fits(&ok_flat));
        let bad_flat = Value::Array(vec![Value::from(0); 100_000]);
        assert!(!webhook_body_fits(&bad_flat));
    }

    #[test]
    fn tw_06_unsafe_integer_and_scalar_equality_edge_cases() {
        let headers = BTreeMap::new();
        let body = json!({
            "code": "0042",
            "n": 42,
            "big": "9007199254740993",
            "bigN": 9007199254740993_i64,
            "nullVal": null,
            "emptyArr": [],
            "tags": ["web", "checkout"]
        });
        let call = WebhookCallView { headers: &headers, body: &body };
        let run = |path: &str, op: WebhookWhereOp, vals: &[&str]| -> bool {
            let f = WebhookFilters {
                where_clauses: vec![WebhookWhereClause {
                    path: path.to_string(),
                    op,
                    values: vals.iter().map(|s| (*s).to_string()).collect(),
                }],
                ..Default::default()
            };
            matches_webhook_call(&f, &call)
        };

        // (a) string vs number equality
        assert!(!run("body.code", WebhookWhereOp::Eq, &["42"]));
        assert!(run("body.code", WebhookWhereOp::Eq, &["0042"]));
        assert!(run("body.n", WebhookWhereOp::Eq, &["0042"]));
        assert!(run("body.n", WebhookWhereOp::Eq, &["42.0"]));

        // (b) unsafe integer > 2^53 - 1
        assert!(!run("body.bigN", WebhookWhereOp::Eq, &["9007199254740993"]));
        assert!(!run("body.bigN", WebhookWhereOp::Eq, &["9007199254740992"]));
        assert!(run("body.bigN", WebhookWhereOp::NotEq, &["9007199254740993"]));
        assert_eq!(webhook_call_subject(Some("body.bigN"), &call), None);
        assert_eq!(
            webhook_call_subject(Some("body.big"), &call).as_deref(),
            Some("9007199254740993")
        );

        // (c) null & missing
        for path in ["body.nullVal", "body.missing"] {
            assert!(run(path, WebhookWhereOp::NotExists, &[]));
            assert!(run(path, WebhookWhereOp::NotEq, &["x"]));
            assert!(!run(path, WebhookWhereOp::Exists, &[]));
            assert!(!run(path, WebhookWhereOp::Eq, &["null"]));
            assert!(!run(path, WebhookWhereOp::Contains, &["n"]));
        }

        // (d) emptyArr
        assert!(run("body.emptyArr", WebhookWhereOp::Exists, &[]));
        assert!(!run("body.emptyArr", WebhookWhereOp::Eq, &["x"]));
        assert!(run("body.emptyArr", WebhookWhereOp::NotEq, &["x"]));

        // (e) tags array
        assert!(run("body.tags", WebhookWhereOp::Eq, &["checkout"]));
        assert!(!run("body.tags", WebhookWhereOp::NotEq, &["checkout"]));
        assert!(run("body.tags", WebhookWhereOp::NotEq, &["other"]));
    }

    #[test]
    fn tw_07_webhook_rule_bounds_and_utf8_2560_byte_cap() {
        let mk_clause = |i: usize| WebhookWhereClause {
            path: format!("body.k{i}"),
            op: WebhookWhereOp::Eq,
            values: vec!["v".to_string()],
        };

        // where 8 ok / 9 err
        let mut f = WebhookFilters {
            where_clauses: (0..8).map(mk_clause).collect(),
            ..Default::default()
        };
        assert!(validate_webhook_filters(&f).is_ok());
        f.where_clauses.push(mk_clause(8));
        assert!(validate_webhook_filters(&f).is_err());

        // op arity checks
        let in_clause = |n: usize| WebhookFilters {
            where_clauses: vec![WebhookWhereClause {
                path: "body.x".to_string(),
                op: WebhookWhereOp::In,
                values: (0..n).map(|i| format!("v{i}")).collect(),
            }],
            ..Default::default()
        };
        assert!(validate_webhook_filters(&in_clause(0)).is_err());
        assert!(validate_webhook_filters(&in_clause(10)).is_ok());
        assert!(validate_webhook_filters(&in_clause(11)).is_err());

        let bad_exists = WebhookFilters {
            where_clauses: vec![WebhookWhereClause {
                path: "body.x".to_string(),
                op: WebhookWhereOp::Exists,
                values: vec!["x".to_string()],
            }],
            ..Default::default()
        };
        assert!(validate_webhook_filters(&bad_exists).is_err());

        // pick 12 ok / 13 err; subject requires subject_from
        let ok_pick = WebhookFilters {
            pick: (0..12).map(|i| format!("body.p{i}")).collect(),
            ..Default::default()
        };
        assert!(validate_webhook_filters(&ok_pick).is_ok());
        let mut bad_pick = ok_pick;
        bad_pick.pick.push("body.p12".to_string());
        assert!(validate_webhook_filters(&bad_pick).is_err());

        let bad_subj = WebhookFilters {
            subject: Some("sub".to_string()),
            ..Default::default()
        };
        assert!(validate_webhook_filters(&bad_subj).is_err());

        // Exact UTF-8 byte boundary: 2560 bytes ok, 2561 bytes err (with multi-byte UTF-8 < 1000 chars!)
        let mut rule = WebhookRuleWire {
            id: "wh-wh_AbCdEfGh12345678".to_string(),
            automation_id: "auto-1".to_string(),
            bot_id: "bot-1".to_string(),
            source: AutomationEventSource::Webhook,
            event: WebhookEventKind::Call,
            rev: 1,
            filters: WebhookFilters {
                where_clauses: (0..4)
                    .map(|i| WebhookWhereClause {
                        path: format!("body.f{i}"),
                        op: WebhookWhereOp::Eq,
                        values: vec!["测".repeat(180)],
                    })
                    .collect(),
                ..Default::default()
            },
            grace_minutes: None,
        };
        let base_bytes = serde_json::to_vec(&rule).expect("ser").len();
        assert!(base_bytes < WEBHOOK_RULE_MAX_BYTES);
        let pad_needed = WEBHOOK_RULE_MAX_BYTES - base_bytes;
        let field_overhead = ",\"textContains\":\"\"".len();
        assert!(pad_needed > field_overhead);
        let filler_len = pad_needed - field_overhead;
        assert!(filler_len < WEBHOOK_MAX_VALUE_LENGTH);
        rule.filters.text_contains = Some("a".repeat(filler_len));
        let exact_json = serde_json::to_string(&rule).expect("ser");
        assert!(exact_json.chars().count() < 1_200);
        assert_eq!(exact_json.len(), 2_560);
        assert_eq!(validate_webhook_rule_bytes(&rule).expect("exact 2560 bytes"), 2_560);
        rule.filters.text_contains = Some("a".repeat(filler_len + 1));
        assert_eq!(validate_webhook_rule_bytes(&rule).unwrap_err().code, BotErrorCode::InvalidArgument);
    }

    #[test]
    fn tw_08_rolling_hour_caps_eviction_account_cap_and_burst_despam() {
        let mut tracker = WebhookHourlyCapTracker::new();
        assert!(tracker.set_hook_cap("wh_custom", 0).is_err());
        assert!(tracker.set_hook_cap("wh_custom", 201).is_err());
        assert!(tracker.set_hook_cap("wh_custom", 200).is_ok());

        let t0: u64 = 10_000;
        for i in 0..200 {
            match tracker.try_admit_start("hook_a", "bot1", t0) {
                WebhookCapDecision::Allowed { hook_used, .. } => assert_eq!(hook_used, i + 1),
                other => panic!("expected allowed at {i}, got {other:?}"),
            }
        }
        // 201st at t0 + 3_599_999 -> capped with retry_at = t0 + 3_600_000 & ONE CapReached notice
        match tracker.try_admit_start("hook_a", "bot1", t0 + 3_599_999) {
            WebhookCapDecision::Capped {
                retry_at_ms,
                hook_capped,
                notice,
                ..
            } => {
                assert!(hook_capped);
                assert_eq!(retry_at_ms, t0 + WEBHOOK_CAP_WINDOW_MS);
                let n = notice.expect("first cap notice");
                assert_eq!(n.condition, WebhookNoticeCondition::CapReached);
                assert_eq!(n.count, 200);
            }
            other => panic!("expected capped, got {other:?}"),
        }
        // 202nd in same window -> NO duplicate notice!
        match tracker.try_admit_start("hook_a", "bot1", t0 + 3_599_999) {
            WebhookCapDecision::Capped { notice, .. } => assert!(notice.is_none()),
            other => panic!("expected capped, got {other:?}"),
        }
        // Account cap 300: 100 more on hook_b allowed, 101st hits account cap!
        for _ in 0..100 {
            assert!(matches!(
                tracker.try_admit_start("hook_b", "bot1", t0 + 100),
                WebhookCapDecision::Allowed { .. }
            ));
        }
        match tracker.try_admit_start("hook_b", "bot1", t0 + 100) {
            WebhookCapDecision::Capped {
                account_capped,
                retry_at_ms,
                ..
            } => {
                assert!(account_capped);
                assert_eq!(retry_at_ms, t0 + WEBHOOK_CAP_WINDOW_MS);
            }
            other => panic!("expected account cap, got {other:?}"),
        }
        // Clock regression does not panic or evict
        assert!(matches!(
            tracker.try_admit_start("hook_a", "bot1", t0 - 5_000),
            WebhookCapDecision::Capped { .. }
        ));
        // Exact eviction boundary t0 + 3_600_000 evicts the 200 calls from t0!
        assert!(matches!(
            tracker.try_admit_start("hook_a", "bot1", t0 + WEBHOOK_CAP_WINDOW_MS),
            WebhookCapDecision::Allowed { hook_used: 1, .. }
        ));

        // Refusal burst: 1..4 None, 5th emits RefusalBurst, 6..50 None
        for _ in 1..=4 {
            assert!(tracker.record_refusal("hook_r", "bot1", 50_000).is_none());
        }
        let burst = tracker
            .record_refusal("hook_r", "bot1", 50_001)
            .expect("5th refusal triggers burst notice");
        assert_eq!(burst.condition, WebhookNoticeCondition::RefusalBurst);
        assert_eq!(burst.count, 5);
        for k in 6..=50 {
            assert!(tracker.record_refusal("hook_r", "bot1", 50_000 + k).is_none());
        }
    }
}

#[cfg(test)]
mod zero_leak_state_and_watch_pr_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tw_09_zero_leak_trigger_event_epoch_fencing_and_redaction() {
        let valid = WebhookTriggerEvent {
            key: "call_AbCdEfGhIj0123456789".to_string(),
            actor_id: "webhook:wh_AbCdEfGh12345678".to_string(),
            subject: Some("issue-42".to_string()),
            occurred_at: 1_700_000_000_000,
            webhook: WebhookCallFields {
                call_id: "call_AbCdEfGhIj0123456789".to_string(),
                epoch: 5,
                host_gen: Some(2),
                sample: Some(true),
            },
            adapter: Some(WebhookAdapterId::Sentry),
            matched_rule_id: Some("wh-wh_AbCdEfGh12345678".to_string()),
        };
        assert!(valid.validate().is_ok());

        // Inspect serialized keys: zero forbidden keys anywhere
        let val = serde_json::to_value(&valid).expect("serialize");
        let forbidden = [
            "text",
            "url",
            "actor",
            "place",
            "fromOwner",
            "from_owner",
            "onOwnerMessage",
            "on_owner_message",
            "pr",
        ];
        let obj = val.as_object().expect("object");
        for k in forbidden {
            assert!(!obj.contains_key(k), "forbidden key {k} leaked!");
        }
        let wh_obj = obj["webhook"].as_object().expect("webhook obj");
        for k in forbidden {
            assert!(!wh_obj.contains_key(k), "forbidden key {k} leaked in webhook!");
        }

        // Injecting EACH forbidden key individually MUST fail deserialization (deny_unknown_fields)
        let injections = [
            ("text", json!("leak")),
            ("url", json!("https://example.com")),
            ("actor", json!("bob")),
            ("place", json!("#ops")),
            ("fromOwner", json!(true)),
            ("onOwnerMessage", json!(true)),
            ("pr", json!("a/b#1")),
        ];
        for (k, v) in injections {
            let mut mutated = obj.clone();
            mutated.insert(k.to_string(), v);
            let res: Result<WebhookTriggerEvent, _> = serde_json::from_value(Value::Object(mutated));
            assert!(res.is_err(), "expected deny_unknown_fields to reject injected key {k}");
        }

        // Validation catches key mismatch and leaked orcawh_ in subject
        let mut mismatch = valid.clone();
        mismatch.key = "call_ZzYyXxWwVv9876543210".to_string();
        assert_eq!(mismatch.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut secret_subj = valid.clone();
        secret_subj.subject = Some(format!("leak orcawh_{}", "a".repeat(43)));
        assert_eq!(secret_subj.validate().unwrap_err().code, BotErrorCode::SecretBlocked);

        // Epoch fencing
        assert_eq!(check_webhook_epoch(4, 5).unwrap_err().code, BotErrorCode::Expired);
        assert!(check_webhook_epoch(5, 5).is_ok());
        assert!(check_webhook_epoch(6, 5).is_ok());
        assert_eq!(advance_webhook_epoch(5), 6);

        // Key redaction: plain, %XX, \u00XX, Markdown \_, and /hooks/wh_<16>/<key>?a=1
        let key43 = "0123456789abcdef0123456789abcdef0123456789a";
        assert!(is_valid_webhook_key(&format!("orcawh_{key43}")));
        assert_eq!(redact_webhook_keys(&format!("tok orcawh_{key43} end")), "tok [orca webhook key] end");
        assert_eq!(redact_webhook_keys(&format!("tok %6frcawh_{key43} end")), "tok [orca webhook key] end");
        assert_eq!(redact_webhook_keys(&format!("tok \\u006frcawh_{key43} end")), "tok [orca webhook key] end");
        assert_eq!(redact_webhook_keys(&format!("tok orcawh\\_{key43} end")), "tok [orca webhook key] end");
        assert_eq!(
            redact_webhook_keys("https://x/hooks/wh_AbCdEfGh12345678/mySecretKey123?a=1"),
            "https://x/hooks/wh_AbCdEfGh12345678/[orca webhook key]?a=1"
        );
    }

    #[test]
    fn tw_10_all_14_webhook_call_states_transitions_and_run_outcomes() {
        let expected = [
            "queued",
            "ignored",
            "paused",
            "delivered",
            "cancelled",
            "waiting_capacity",
            "started",
            "folded",
            "refused",
            "missed",
            "unavailable",
            "completed",
            "failed",
            "unverifiable",
        ];
        assert_eq!(WebhookCallState::ALL.len(), 14);
        for (st, wire) in WebhookCallState::ALL.into_iter().zip(expected) {
            assert_eq!(st.as_str(), wire);
            assert_eq!(WebhookCallState::parse(wire), Some(st));
            let s = serde_json::to_string(&st).expect("ser");
            assert_eq!(s, format!("\"{wire}\""));
            let back: WebhookCallState = serde_json::from_str(&s).expect("de");
            assert_eq!(back, st);
        }

        // Legal forward transitions
        assert!(can_transition_webhook_call_state(WebhookCallState::Queued, WebhookCallState::Delivered));
        assert!(can_transition_webhook_call_state(WebhookCallState::Delivered, WebhookCallState::Started));
        assert!(can_transition_webhook_call_state(WebhookCallState::Started, WebhookCallState::Completed));

        // Illegal terminal reversals
        let illegal = [
            (WebhookCallState::Completed, WebhookCallState::Started),
            (WebhookCallState::Refused, WebhookCallState::Delivered),
            (WebhookCallState::Cancelled, WebhookCallState::Queued),
            (WebhookCallState::Ignored, WebhookCallState::Started),
            (WebhookCallState::Failed, WebhookCallState::Completed),
            (WebhookCallState::Unverifiable, WebhookCallState::Completed),
        ];
        for (from, to) in illegal {
            assert!(!can_transition_webhook_call_state(from, to));
        }

        // settled_run_outcome across all 9 AutomationRunStatus variants
        assert_eq!(AutomationRunStatus::ALL.len(), 9);
        assert_eq!(settled_run_outcome(AutomationRunStatus::Pending), None);
        assert_eq!(settled_run_outcome(AutomationRunStatus::Dispatching), None);
        assert_eq!(settled_run_outcome(AutomationRunStatus::Dispatched), None);
        assert_eq!(settled_run_outcome(AutomationRunStatus::Completed), Some(WebhookCallState::Completed));
        assert_eq!(settled_run_outcome(AutomationRunStatus::DispatchFailed), Some(WebhookCallState::Failed));
        assert_eq!(settled_run_outcome(AutomationRunStatus::SkippedMissed), Some(WebhookCallState::Missed));
        assert_eq!(settled_run_outcome(AutomationRunStatus::SkippedUnavailable), Some(WebhookCallState::Unavailable));
        assert_eq!(settled_run_outcome(AutomationRunStatus::SkippedPrecheck), Some(WebhookCallState::Unavailable));
        assert_eq!(
            settled_run_outcome(AutomationRunStatus::SkippedNeedsInteractiveAuth),
            Some(WebhookCallState::Unavailable)
        );
    }

    #[test]
    fn tw_11_one_shot_wait_and_watch_pr_policy_boundaries() {
        // (a) claims_the_run & once_claimed
        for st in [
            AutomationRunStatus::SkippedPrecheck,
            AutomationRunStatus::SkippedMissed,
            AutomationRunStatus::SkippedUnavailable,
            AutomationRunStatus::SkippedNeedsInteractiveAuth,
        ] {
            assert!(!claims_the_run(st));
        }
        for st in [
            AutomationRunStatus::Pending,
            AutomationRunStatus::Dispatching,
            AutomationRunStatus::Dispatched,
            AutomationRunStatus::Completed,
            AutomationRunStatus::DispatchFailed,
        ] {
            assert!(claims_the_run(st));
        }
        // Manual Completed run NEVER claims the one-shot wait
        let manual_only = [OneShotRunSummary {
            trigger: AutomationRunTrigger::Manual,
            status: AutomationRunStatus::Completed,
        }];
        assert!(!once_claimed(&manual_only));
        assert!(!once_run_finished(&manual_only));

        let event_done = [OneShotRunSummary {
            trigger: AutomationRunTrigger::Event,
            status: AutomationRunStatus::Completed,
        }];
        assert!(once_claimed(&event_done));
        assert!(once_run_finished(&event_done));

        // (b) WAIT_MAX_DEADLINE_MS & overflow-safe validation
        assert_eq!(WAIT_MAX_DEADLINE_MS, 31_104_000_000);
        let created = 1_000_000u64;
        assert!(OneShotWait { by: Some(created + WAIT_MAX_DEADLINE_MS) }
            .validate(created, true, false, false)
            .is_ok());
        assert!(OneShotWait { by: Some(created + WAIT_MAX_DEADLINE_MS + 1) }
            .validate(created, true, false, false)
            .is_err());
        assert!(OneShotWait { by: Some(created) }
            .validate(created, true, false, true)
            .is_err());
        assert!(OneShotWait { by: Some(u64::MAX) }
            .validate(u64::MAX - 10, true, false, false)
            .is_ok());

        // pending_waits sorts soonest deadline first, no-deadline last, excludes claimed
        let pending = pending_waits(&[
            PendingWaitEntry { id: "no-by".to_string(), wait: OneShotWait { by: None }, runs: vec![] },
            PendingWaitEntry { id: "claimed".to_string(), wait: OneShotWait { by: Some(1) }, runs: event_done.to_vec() },
            PendingWaitEntry { id: "late".to_string(), wait: OneShotWait { by: Some(500) }, runs: vec![] },
            PendingWaitEntry { id: "soon".to_string(), wait: OneShotWait { by: Some(100) }, runs: vec![] },
        ]);
        let ids: Vec<&str> = pending.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["soon", "late", "no-by"]);

        // (c) parse_watched_pr
        assert!(parse_watched_pr("org/repo#0").is_err());
        assert!(parse_watched_pr("org/repo#01").is_err());
        assert_eq!(
            parse_watched_pr("https://github.com/org/repo/pull/42/checks?x=1#top").expect("pr url"),
            "org/repo#42"
        );
        assert_eq!(parse_watched_pr("Org/Repo#7").expect("short"), "Org/Repo#7");

        // (d) parse_watch_duration
        assert!(parse_watch_duration("0m").is_err());
        assert!(parse_watch_duration("0h0m").is_err());
        assert!(parse_watch_duration("99999999999999999999h").is_err());
        assert_eq!(parse_watch_duration("1h30m").expect("dur"), 5_400_000);
        assert_eq!(parse_watch_duration("8640h").expect("max"), WAIT_MAX_DEADLINE_MS);
        assert!(parse_watch_duration("8640h1m").is_err());

        // (e) watch_pr_events & evaluate_watch_pr_step (strict 2-attempt ceiling)
        assert_eq!(watch_pr_events(true).len(), 7);
        assert_eq!(watch_pr_events(false).len(), 8);
        assert_eq!(
            evaluate_watch_pr_step(1, WatchPrWakeOutcome::NeedsChange { authorized: true }, 2),
            WatchPrStepAction::DispatchWorkerFix {
                attempt: 1,
                next_attempt: 2,
                rearm_after_worker_push: true,
            }
        );
        assert_eq!(
            evaluate_watch_pr_step(2, WatchPrWakeOutcome::NeedsChange { authorized: true }, 2),
            WatchPrStepAction::DispatchWorkerFix {
                attempt: 2,
                next_attempt: 3,
                rearm_after_worker_push: true,
            }
        );
        assert_eq!(
            evaluate_watch_pr_step(3, WatchPrWakeOutcome::NeedsChange { authorized: true }, 2),
            WatchPrStepAction::EscalateToOwnerAndStop { attempt: 3 }
        );
        assert_eq!(
            evaluate_watch_pr_step(0, WatchPrWakeOutcome::NeedsChange { authorized: true }, 2),
            WatchPrStepAction::EscalateToOwnerAndStop { attempt: 0 }
        );
        assert_eq!(
            evaluate_watch_pr_step(1, WatchPrWakeOutcome::NeedsChange { authorized: false }, 2),
            WatchPrStepAction::NotifyOwnerAndRearmSameAttempt { attempt: 1 }
        );

        // Event folding retains at most 50 folded keys and keeps strictly newest occurred_at
        let mut latest = FoldedEventEntry { key: "k0".to_string(), occurred_at: 100 };
        let mut folded = Vec::new();
        for i in 1..=60u64 {
            let (l, f) = with_folded_event(
                latest,
                folded,
                FoldedEventEntry { key: format!("k{i}"), occurred_at: 100 + i },
            );
            latest = l;
            folded = f;
        }
        assert_eq!(latest.key, "k60");
        assert_eq!(folded.len(), MAX_RUN_EVENT_KEYS);
        assert!(run_has_event(Some(&latest.key), &folded, "k60"));
        assert!(run_has_event(Some(&latest.key), &folded, "k59"));
        assert!(!run_has_event(Some(&latest.key), &folded, "k0"));
    }

    #[test]
    fn tw_12_remediation_regressions() {
        // Finding #11: parse_webhook_path rejects mixed/uppercase prototype segments under headers.*
        for bad_hdr in [
            "headers.__PROTO__",
            "headers.Prototype",
            "headers.PROTOTYPE",
            "headers.CONSTRUCTOR",
            "headers.Constructor",
        ] {
            assert_eq!(
                parse_webhook_path(bad_hdr).unwrap_err().code,
                BotErrorCode::InvalidArgument,
                "expected forbidden header segment rejected: {bad_hdr}"
            );
        }

        // Finding #12: redact_webhook_keys idempotency & already-redacted subject validation
        let raw_url = "https://ex.com/hooks/wh_AbCdEfGh12345678/secret?a=1#b";
        let r1 = redact_webhook_keys(raw_url);
        assert_eq!(r1, "https://ex.com/hooks/wh_AbCdEfGh12345678/[orca webhook key]?a=1#b");
        assert_eq!(redact_webhook_keys(&r1), r1);
        assert_eq!(redact_webhook_keys(&redact_webhook_keys(&r1)), r1);

        let redacted_ev = WebhookTriggerEvent {
            key: "call_AbCdEfGhIj0123456789".to_string(),
            actor_id: "webhook:wh_AbCdEfGh12345678".to_string(),
            subject: Some(r1),
            occurred_at: 1_700_000_000_000,
            webhook: WebhookCallFields {
                call_id: "call_AbCdEfGhIj0123456789".to_string(),
                epoch: 1,
                host_gen: None,
                sample: None,
            },
            adapter: Some(WebhookAdapterId::Standard),
            matched_rule_id: None,
        };
        assert!(redacted_ev.validate().is_ok());

        // Finding #13a: Clock-skew eviction does not poison later expired entries
        let mut skew_tracker = WebhookHourlyCapTracker::new();
        skew_tracker.set_hook_cap("hs", 2).expect("cap 2");
        assert!(matches!(
            skew_tracker.try_admit_start("hs", "b", 5_000_000),
            WebhookCapDecision::Allowed { hook_used: 1, .. }
        ));
        assert!(matches!(
            skew_tracker.try_admit_start("hs", "b", 4_900_000),
            WebhookCapDecision::Allowed { hook_used: 2, .. }
        ));
        assert!(matches!(
            skew_tracker.try_admit_start("hs", "b", 4_900_000 + WEBHOOK_CAP_WINDOW_MS),
            WebhookCapDecision::Allowed { hook_used: 2, .. }
        ));

        // Finding #13b: Lowered cap computes exact retry_at_ms so retrying at retry_at_ms succeeds
        let mut lower_tracker = WebhookHourlyCapTracker::new();
        lower_tracker.set_hook_cap("hd", 3).expect("cap 3");
        for ts in [10_000u64, 20_000, 30_000] {
            assert!(matches!(
                lower_tracker.try_admit_start("hd", "b", ts),
                WebhookCapDecision::Allowed { .. }
            ));
        }
        lower_tracker.set_hook_cap("hd", 1).expect("cap 1");
        let retry_at = match lower_tracker.try_admit_start("hd", "b", 35_000) {
            WebhookCapDecision::Capped { retry_at_ms, .. } => retry_at_ms,
            other => panic!("expected capped, got {other:?}"),
        };
        assert_eq!(retry_at, 30_000 + WEBHOOK_CAP_WINDOW_MS);
        assert!(matches!(
            lower_tracker.try_admit_start("hd", "b", retry_at - 1),
            WebhookCapDecision::Capped { .. }
        ));
        assert!(matches!(
            lower_tracker.try_admit_start("hd", "b", retry_at),
            WebhookCapDecision::Allowed { hook_used: 1, hook_cap: 1, .. }
        ));

        // Finding #14: validate_webhook_rule_bytes rejects wrong source & blank automation_id/bot_id
        let base_rule = WebhookRuleWire {
            id: "wh-wh_AbCdEfGh12345678".to_string(),
            automation_id: "auto-1".to_string(),
            bot_id: "bot-1".to_string(),
            source: AutomationEventSource::Webhook,
            event: WebhookEventKind::Call,
            rev: 1,
            filters: WebhookFilters::default(),
            grace_minutes: None,
        };
        assert!(validate_webhook_rule_bytes(&base_rule).is_ok());

        let mut bad_src = base_rule.clone();
        bad_src.source = AutomationEventSource::Gmail;
        assert_eq!(
            validate_webhook_rule_bytes(&bad_src).unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        let mut blank_auto = base_rule.clone();
        blank_auto.automation_id = "   ".to_string();
        assert_eq!(
            validate_webhook_rule_bytes(&blank_auto).unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        let mut blank_bot = base_rule;
        blank_bot.bot_id = "".to_string();
        assert_eq!(
            validate_webhook_rule_bytes(&blank_bot).unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        // Finding #15: AutomationBotTarget agent_setup serde alias + target.validate()
        let target_json = json!({
            "botId": "bot-1",
            "runsIn": { "kind": "new" },
            "resultsTo": { "kind": "chat" },
            "agentSetup": {
                "model": "claude-sonnet-4-5",
                "effort": "high",
                "extraArgs": ["--verbose"]
            }
        });
        let target: AutomationBotTarget =
            serde_json::from_value(target_json).expect("deserialize target with agentSetup");
        assert!(target.validate().is_ok());
        assert_eq!(
            target.agent_setup.as_ref().and_then(|c| c.model.as_deref()),
            Some("claude-sonnet-4-5")
        );

        let mut bad_bot_id = target.clone();
        bad_bot_id.bot_id = "  ".to_string();
        assert_eq!(
            validate_automation_event_triggers(&[], Some(&bad_bot_id), 1_700_000_000_000, None)
                .unwrap_err()
                .code,
            BotErrorCode::InvalidArgument
        );

        let mut bad_chat_sess = target.clone();
        bad_chat_sess.runs_in = AutomationBotRunsIn::Chat {
            session_id: "   ".to_string(),
            title: None,
            home_if_gone: None,
            to_conversation: None,
        };
        assert_eq!(
            validate_automation_event_triggers(&[], Some(&bad_chat_sess), 1_700_000_000_000, None)
                .unwrap_err()
                .code,
            BotErrorCode::InvalidArgument
        );

        // Finding #16a: validate_automation_event_triggers one-shot invariants
        let wait_no_by = OneShotWait { by: None };
        let wait_with_by = OneShotWait { by: Some(1_800_000_000_000) };
        assert_eq!(
            validate_automation_event_triggers(
                &[],
                None,
                EVENT_ONLY_SCHEDULE_DTSTART_MS,
                Some(&wait_no_by),
            )
            .unwrap_err()
            .code,
            BotErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_automation_event_triggers(
                &[],
                Some(&target),
                EVENT_ONLY_SCHEDULE_DTSTART_MS,
                Some(&wait_no_by),
            )
            .unwrap_err()
            .code,
            BotErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_automation_event_triggers(
                &[],
                Some(&target),
                1_700_000_000_000,
                Some(&wait_with_by),
            )
            .unwrap_err()
            .code,
            BotErrorCode::InvalidArgument
        );
        assert!(validate_automation_event_triggers(
            &[],
            Some(&target),
            EVENT_ONLY_SCHEDULE_DTSTART_MS,
            Some(&wait_with_by),
        )
        .is_ok());

        // Finding #16b: with_folded_event guarantees folded_keys disjoint from latest.key
        let (l1, f1) = with_folded_event(
            FoldedEventEntry { key: "k1".to_string(), occurred_at: 200 },
            Vec::new(),
            FoldedEventEntry { key: "k2".to_string(), occurred_at: 100 },
        );
        assert_eq!(l1.key, "k1");
        assert_eq!(f1, vec!["k2".to_string()]);
        let (l2, f2) = with_folded_event(
            l1,
            f1,
            FoldedEventEntry { key: "k2".to_string(), occurred_at: 300 },
        );
        assert_eq!(l2.key, "k2");
        assert_eq!(f2, vec!["k1".to_string()]);
        assert!(!f2.contains(&l2.key));

        // Finding #16c: WebhookTriggerEvent rejects whitespace-only subject
        let mut ws_subj = redacted_ev;
        ws_subj.subject = Some("   ".to_string());
        assert_eq!(ws_subj.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        // Finding #16d: validate_google_trigger rejects empty/whitespace filter strings
        let bad_google = [
            AutomationGoogleTrigger::Gmail {
                id: "g1".to_string(),
                account: Some("   ".to_string()),
                from: vec![],
                to: vec![],
                subject_contains: None,
                label: None,
            },
            AutomationGoogleTrigger::Gmail {
                id: "g1".to_string(),
                account: None,
                from: vec!["".to_string()],
                to: vec![],
                subject_contains: None,
                label: None,
            },
            AutomationGoogleTrigger::Gmail {
                id: "g1".to_string(),
                account: None,
                from: vec![],
                to: vec!["  ".to_string()],
                subject_contains: None,
                label: None,
            },
            AutomationGoogleTrigger::Gmail {
                id: "g1".to_string(),
                account: None,
                from: vec![],
                to: vec![],
                subject_contains: Some(" ".to_string()),
                label: None,
            },
            AutomationGoogleTrigger::Gmail {
                id: "g1".to_string(),
                account: None,
                from: vec![],
                to: vec![],
                subject_contains: None,
                label: Some("".to_string()),
            },
            AutomationGoogleTrigger::Calendar {
                id: "c1".to_string(),
                account: Some(" ".to_string()),
                calendar: None,
                minutes_before: 10,
            },
            AutomationGoogleTrigger::Calendar {
                id: "c1".to_string(),
                account: None,
                calendar: Some("".to_string()),
                minutes_before: 10,
            },
        ];
        for g in bad_google {
            assert_eq!(
                validate_google_trigger(&g).unwrap_err().code,
                BotErrorCode::InvalidArgument
            );
        }
    }
}
