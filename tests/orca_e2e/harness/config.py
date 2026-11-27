"""Canonical Orca E2E specification constants and repository paths.

All values are derived verbatim from PROJECT.md §1, orca_core_spec.md,
and orca_bots_and_designs_spec.md. Pure Python 3.12 stdlib only.
"""

from __future__ import annotations

import os
import shutil
from pathlib import Path

SUITE_ROOT: Path = Path(__file__).resolve().parent.parent
REPO_ROOT: Path = SUITE_ROOT.parent.parent
SERVER_BIN: Path = REPO_ROOT / "target" / "debug" / "gpui_telegram_server"
CLI_BIN: Path = REPO_ROOT / "target" / "debug" / "fabushi"
PYTEST_BIN: str = (
    "/opt/pipx/bin/pytest"
    if os.path.isfile("/opt/pipx/bin/pytest") and os.access("/opt/pipx/bin/pytest", os.X_OK)
    else (shutil.which("pytest") or "/usr/bin/pytest")
)

TARGET_RELEASE_VERSION: str = "1.7.0"
TARGET_RELEASE_TAG: str = "v1.7.0"
TARGET_RELEASE_TITLE: str = "Fabushi v1.7.0"

# 22 Native-Chat Bot Agents (PROJECT.md §1, orca_bots_and_designs_spec.md §1.1)
BOT_AGENTS: tuple[str, ...] = (
    "claude",
    "codex",
    "grok",
    "opencode",
    "omp",
    "pi",
    "antigravity",
    "cursor",
    "hermes",
    "copilot",
    "opencode2",
    "kimi",
    "kiro",
    "aug",
    "goose",
    "kilo",
    "mimo-code",
    "droid",
    "qwen-code",
    "cline",
    "devin",
    "codebuddy",
)

LOOK_AROUND_AGENTS: tuple[str, ...] = ("claude", "codex")
TIDY_UP_AGENTS: tuple[str, ...] = ("claude", "codex")

BOT_MASCOTS_20: tuple[str, ...] = tuple(f"{chr(ord('A') + i)}1" for i in range(20))
BOT_OUTFITS_9: tuple[str, ...] = (
    "artist",
    "detective",
    "hard-hat",
    "night-shift",
    "party",
    "reviewer",
    "scientist",
    "security",
    "tie",
)

BOT_ITEM_STATES_6: tuple[str, ...] = (
    "planned",
    "working",
    "waiting",
    "needs_you",
    "done",
    "canceled",
)

BOT_CARD_KINDS_10: tuple[str, ...] = (
    "memoryUpdated",
    "connectorOffer",
    "importPlan",
    "importDone",
    "botRequest",
    "googleSetup",
    "composioSetup",
    "createOffer",
    "toolAdded",
    "webhook",
)

# 29 Bot Error Codes (orca_core::BotErrorCode, PROJECT.md §1)
BOT_ERROR_CODES: tuple[str, ...] = (
    "bot_name_taken",
    "bot_not_found",
    "bot_folder_not_owned",
    "bot_avatar_changed",
    "bot_avatar_invalid",
    "invalid_argument",
    "bots_disabled",
    "bot_chat_not_found",
    "bot_paused",
    "bot_self",
    "bot_request_type",
    "bot_request_worker",
    "not_found",
    "item_closed",
    "not_owner",
    "items_read_only",
    "items_unavailable",
    "item_changed",
    "question_closed",
    "bot_package_newer_schema",
    "bot_package_duplicate_part",
    "bot_package_path_not_allowed",
    "secret_blocked",
    "lineage_parent_changed",
    "bot_offer_already_answered",
    "expired",
    "not_this_host",
    "full",
    "granted_not_added",
)

# 17 IDE / Orchestration / Security Error Codes (orca_core::OrcaIdeErrorCode, PROJECT.md §1)
IDE_ERROR_CODES: tuple[str, ...] = (
    "unknown_task",
    "unknown_dispatch",
    "stale_dispatch",
    "mailbox_is_local",
    "server_mail_unsupported",
    "tab_not_found",
    "system_shortcut_blocked",
    "approval_required",
    "plugin_system_disabled",
    "emulator_unsupported",
    "emulator_input_blocked",
    "wait_timeout",
    "yaml_duplicate_key",
    "yaml_size_exceeded",
    "yaml_alias_limit",
    "copy_budget_exceeded",
    "hibernation_blocked",
)

# Numeric Caps & Protocol Constants (PROJECT.md §1 & surveys)
BOT_NAME_MAX_LEN: int = 64
BOT_MEMORY_INDEX_LINE_CAP: int = 200
READD_GUARD_DAYS: int = 14
TIDY_UP_TARGET_FACTS: int = 60
TIDY_UP_MAX_NEW_FACTS: int = 6
TIDY_UP_MAX_FILED: int = 20
BOT_MAX_MEMBERS: int = 200
BOT_MAX_CONNECTOR_WORKSPACES: int = 8
BOT_GROUP_MAX_REMOTE_MEMBERS: int = 31
BOT_ITEM_TITLE_MAX_LEN: int = 200
BOT_ITEM_DESCRIPTION_MAX_LEN: int = 20_000
BOT_ITEM_DONE_MEANS_MAX_LEN: int = 2_000
BOT_ITEM_TEXT_MAX_LEN: int = 8_000
BOT_ITEM_WAIT_MAX_LEN: int = 200
BOT_ITEM_MAX_LINKS: int = 1_000
BOT_ITEM_ASK_MAX_QUESTIONS: int = 5
BOT_ITEM_ASK_MAX_OPTIONS: int = 8
LOOK_AROUND_MAX_SUGGESTIONS: int = 5
WEBHOOK_HOOK_HOURLY_CAP: int = 200
WEBHOOK_ACCOUNT_HOURLY_CAP: int = 300
WEBHOOK_MAX_WHERE: int = 8
WEBHOOK_MAX_VALUES: int = 10
WEBHOOK_MAX_PICK: int = 12
WEBHOOK_MAX_RULE_BYTES: int = 2_560
WEBHOOK_CALL_STATES_COUNT: int = 14
ORCA_YAML_MAX_BYTES: int = 262_144
ORCA_YAML_MAX_ALIAS_COUNT: int = 100
ORCA_YAML_MAX_SHARED_DIRS: int = 100
ORCA_YAML_MAX_DEFAULT_TABS: int = 256
WORKTREE_INCLUDE_MAX_BYTES: int = 2_147_483_648
WORKTREE_INCLUDE_MAX_ENTRIES: int = 50_000
WORKTREE_INCLUDE_MAX_PATHS: int = 1_000
CLOSED_TAB_TOMBSTONE_DAYS: int = 30
RUNTIME_PROTOCOL_VERSION: int = 3
MIN_COMPATIBLE_PROTOCOL_VERSION: int = 2
E2EE_V2_KEY_SCHEDULE_BYTES: int = 96
WAIT_DEFAULT_EXPIRY_DAYS: int = 30
WAIT_MAX_DEADLINE_DAYS: int = 365
WATCH_PR_MAX_FIX_ATTEMPTS: int = 2
SHARED_MCP_FAILED_START_DEBOUNCE_MS: int = 10_000
PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC: int = 2
PLUGIN_APPROVAL_REQUIRED_EXIT_CODE: int = 3
WS_CLOSE_SUPERSEDED: int = 4409
WS_CLOSE_MESSAGE_TOO_BIG: int = 1009

# 24 Built-In Bot Templates grouped by 6 categories (orca_bots_and_designs_spec.md §1.10)
TEMPLATE_CATEGORIES_6: dict[str, tuple[str, ...]] = {
    "coding": (
        "pr-review",
        "bug-investigation",
        "dependency-updates",
        "unreviewed-work",
        "workspace-cleanup",
        "security-check",
    ),
    "releases": (
        "release",
        "release-notes",
        "ci-failures",
        "deploy-watch",
    ),
    "support": (
        "support",
        "feedback",
        "bug-intake",
        "issue-triage",
    ),
    "planning": (
        "briefing",
        "docs-upkeep",
        "spec-drafts",
        "backlog-cleanup",
    ),
    "ops": (
        "incident-notes",
        "oncall-handoff",
    ),
    "personal": (
        "followup",
        "standup",
        "email-digest",
        "meeting-prep",
    ),
}

TEMPLATE_IDS_24: tuple[str, ...] = tuple(
    tid for category_ids in TEMPLATE_CATEGORIES_6.values() for tid in category_ids
)

# 13 Bundled Template Skills (orca_bots_and_designs_spec.md §1.10)
BUNDLED_SKILLS_13: tuple[str, ...] = (
    "pr-review-checklist",
    "bug-reproduction",
    "ci-failure-triage",
    "release-readiness",
    "release-notes-style",
    "dependency-upgrade",
    "security-checklist",
    "issue-triage",
    "feedback-themes",
    "status-brief",
    "spec-format",
    "incident-timeline",
    "workspace-hygiene",
)

# 6 Trigger Sources -> 31 Event Kinds (orca_bots_and_designs_spec.md §1.9)
TRIGGER_SOURCES_6_KINDS_31: dict[str, tuple[str, ...]] = {
    "slack": (
        "mentioned",
        "message",
        "reaction",
    ),
    "github": (
        "issue_opened",
        "pr_opened",
        "pr_pushed",
        "pr_merged",
        "comment",
        "review_comment",
        "review",
        "mentioned",
        "review_requested",
        "assigned",
        "ci_failed",
        "ci_passed",
        "ci_finished",
        "pr_conflicted",
        "pr_closed",
        "pr_ready",
        "labeled",
        "issue_closed",
        "deploy_finished",
        "security_alert",
        "workflow_finished",
    ),
    "linear": (
        "assigned",
        "mentioned",
        "issue_created",
        "issue_moved",
    ),
    "gmail": (
        "new_email",
    ),
    "calendar": (
        "starting_soon",
    ),
    "webhook": (
        "call",
    ),
}

WEBHOOK_ADAPTERS_6: tuple[str, ...] = (
    "standard",
    "github",
    "sentry",
    "linear",
    "pagerduty",
    "stripe",
)

WEBHOOK_FILTER_OPS_6: tuple[str, ...] = (
    "=",
    "!=",
    "in",
    "contains",
    "exists",
    "!exists",
)

TAB_CONTENT_TYPES_10: tuple[str, ...] = (
    "terminal",
    "editor",
    "diff",
    "conflict-review",
    "check-details",
    "git-graph",
    "agent-session",
    "browser",
    "simulator",
    "room",
)

ORCHESTRATION_MESSAGE_TYPES_9: tuple[str, ...] = (
    "status",
    "dispatch",
    "worker_done",
    "merge_ready",
    "escalation",
    "handoff",
    "decision_gate",
    "question",
    "heartbeat",
)

PLUGIN_TIERS_4: tuple[str, ...] = (
    "data",
    "declarative",
    "sandboxed-ui",
    "trusted-worker",
)

USAGE_PROVIDERS_9: tuple[str, ...] = (
    "claude",
    "codex",
    "opencode-go",
    "kimi-code",
    "minimax",
    "grok",
    "antigravity",
    "cursor",
    "glm-coding-plan",
)

ALL_FEATURES_24: tuple[str, ...] = tuple(f"f{i:02d}" for i in range(1, 25))

ALL_EDGE_CASES_40: tuple[str, ...] = (
    tuple(f"SM1-{i:02d}" for i in range(1, 15))
    + tuple(f"SM2-{i:02d}" for i in range(1, 27))
)
