"""Public exports for the Orca Pure-Rust opaque-box E2E test harness."""

from __future__ import annotations

import importlib
import sys
from pathlib import Path

_SHORT_ROOT = "harness"
_FULL_ROOT = "tests.orca_e2e.harness"
_SUBMODULES = ("config", "fixtures", "mock_servers", "runners", "transport")
_ALT_ROOT = (
    _FULL_ROOT
    if __name__ == _SHORT_ROOT
    else (_SHORT_ROOT if __name__ == _FULL_ROOT else None)
)

if _ALT_ROOT is not None and _ALT_ROOT in sys.modules:
    _canonical = sys.modules[_ALT_ROOT]
    sys.modules[__name__] = _canonical
    for _sub in _SUBMODULES:
        _sub_mod = sys.modules.get(f"{_ALT_ROOT}.{_sub}") or getattr(_canonical, _sub, None)
        if _sub_mod is not None:
            sys.modules[f"{__name__}.{_sub}"] = _sub_mod
    for _k, _v in _canonical.__dict__.items():
        if _k not in ("__name__", "__package__", "__spec__"):
            globals()[_k] = _v
else:
    from . import config, fixtures, mock_servers, runners, transport
    from .config import (
        ALL_EDGE_CASES_40,
        ALL_FEATURES_24,
        BOT_AGENTS,
        BOT_CARD_KINDS_10,
        BOT_ERROR_CODES,
        BOT_GROUP_MAX_REMOTE_MEMBERS,
        BOT_ITEM_ASK_MAX_OPTIONS,
        BOT_ITEM_ASK_MAX_QUESTIONS,
        BOT_ITEM_DESCRIPTION_MAX_LEN,
        BOT_ITEM_DONE_MEANS_MAX_LEN,
        BOT_ITEM_MAX_LINKS,
        BOT_ITEM_STATES_6,
        BOT_ITEM_TEXT_MAX_LEN,
        BOT_ITEM_TITLE_MAX_LEN,
        BOT_ITEM_WAIT_MAX_LEN,
        BOT_MASCOTS_20,
        BOT_MAX_CONNECTOR_WORKSPACES,
        BOT_MAX_MEMBERS,
        BOT_MEMORY_INDEX_LINE_CAP,
        BOT_NAME_MAX_LEN,
        BOT_OUTFITS_9,
        BUNDLED_SKILLS_13,
        CLI_BIN,
        CLOSED_TAB_TOMBSTONE_DAYS,
        E2EE_V2_KEY_SCHEDULE_BYTES,
        IDE_ERROR_CODES,
        LOOK_AROUND_AGENTS,
        LOOK_AROUND_MAX_SUGGESTIONS,
        MIN_COMPATIBLE_PROTOCOL_VERSION,
        ORCA_YAML_MAX_ALIAS_COUNT,
        ORCA_YAML_MAX_BYTES,
        ORCA_YAML_MAX_DEFAULT_TABS,
        ORCA_YAML_MAX_SHARED_DIRS,
        ORCHESTRATION_MESSAGE_TYPES_9,
        PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC,
        PLUGIN_APPROVAL_REQUIRED_EXIT_CODE,
        PLUGIN_TIERS_4,
        PYTEST_BIN,
        READD_GUARD_DAYS,
        REPO_ROOT,
        RUNTIME_PROTOCOL_VERSION,
        SERVER_BIN,
        SHARED_MCP_FAILED_START_DEBOUNCE_MS,
        SUITE_ROOT,
        TAB_CONTENT_TYPES_10,
        TARGET_RELEASE_TAG,
        TARGET_RELEASE_TITLE,
        TARGET_RELEASE_VERSION,
        TEMPLATE_CATEGORIES_6,
        TEMPLATE_IDS_24,
        TIDY_UP_AGENTS,
        TIDY_UP_MAX_FILED,
        TIDY_UP_MAX_NEW_FACTS,
        TIDY_UP_TARGET_FACTS,
        TRIGGER_SOURCES_6_KINDS_31,
        USAGE_PROVIDERS_9,
        WAIT_DEFAULT_EXPIRY_DAYS,
        WAIT_MAX_DEADLINE_DAYS,
        WATCH_PR_MAX_FIX_ATTEMPTS,
        WEBHOOK_ACCOUNT_HOURLY_CAP,
        WEBHOOK_ADAPTERS_6,
        WEBHOOK_CALL_STATES_COUNT,
        WEBHOOK_FILTER_OPS_6,
        WEBHOOK_HOOK_HOURLY_CAP,
        WEBHOOK_MAX_PICK,
        WEBHOOK_MAX_RULE_BYTES,
        WEBHOOK_MAX_VALUES,
        WEBHOOK_MAX_WHERE,
        WORKTREE_INCLUDE_MAX_BYTES,
        WORKTREE_INCLUDE_MAX_ENTRIES,
        WORKTREE_INCLUDE_MAX_PATHS,
        WS_CLOSE_MESSAGE_TOO_BIG,
        WS_CLOSE_SUPERSEDED,
    )
    from .fixtures import BotHomeFixture, GitRepoFixture, SqliteFixtureHelper, TempWorkspace
    from .mock_servers import (
        MockMcpUpstreamServer,
        MockOAuthServer,
        MockWebhookSink,
        RecordedRequest,
        pkce_s256_challenge,
    )
    from .runners import (
        AuthSession,
        CliResult,
        OrcaCliRunner,
        OrcaServerHandle,
        allocate_free_port,
    )
    from .transport import HttpResponse, OrcaApiClient, RawHttpClient, WsClient, WsFrame

    _self_mod = sys.modules[__name__]
    _loaded_subs = {
        "config": config,
        "fixtures": fixtures,
        "mock_servers": mock_servers,
        "runners": runners,
        "transport": transport,
    }
    for _sub, _mod in _loaded_subs.items():
        sys.modules.setdefault(f"{__name__}.{_sub}", _mod)

    if __name__ == _FULL_ROOT:
        sys.modules[_SHORT_ROOT] = _self_mod
        for _sub, _mod in _loaded_subs.items():
            sys.modules[f"{_SHORT_ROOT}.{_sub}"] = _mod
    elif __name__ == _SHORT_ROOT:
        _suite_dir = Path(__file__).resolve().parent.parent
        try:
            _parent = importlib.import_module("tests.orca_e2e")
        except ImportError:
            _parent = None
        if _parent is not None:
            _parent_paths = {
                Path(p).resolve() for p in getattr(_parent, "__path__", ())
            }
            if _suite_dir in _parent_paths:
                setattr(_parent, "harness", _self_mod)
                sys.modules[_FULL_ROOT] = _self_mod
                for _sub, _mod in _loaded_subs.items():
                    sys.modules[f"{_FULL_ROOT}.{_sub}"] = _mod

__all__ = [
    "ALL_EDGE_CASES_40",
    "ALL_FEATURES_24",
    "AuthSession",
    "BOT_AGENTS",
    "BOT_CARD_KINDS_10",
    "BOT_ERROR_CODES",
    "BOT_GROUP_MAX_REMOTE_MEMBERS",
    "BOT_ITEM_ASK_MAX_OPTIONS",
    "BOT_ITEM_ASK_MAX_QUESTIONS",
    "BOT_ITEM_DESCRIPTION_MAX_LEN",
    "BOT_ITEM_DONE_MEANS_MAX_LEN",
    "BOT_ITEM_MAX_LINKS",
    "BOT_ITEM_STATES_6",
    "BOT_ITEM_TEXT_MAX_LEN",
    "BOT_ITEM_TITLE_MAX_LEN",
    "BOT_ITEM_WAIT_MAX_LEN",
    "BOT_MASCOTS_20",
    "BOT_MAX_CONNECTOR_WORKSPACES",
    "BOT_MAX_MEMBERS",
    "BOT_MEMORY_INDEX_LINE_CAP",
    "BOT_NAME_MAX_LEN",
    "BOT_OUTFITS_9",
    "BUNDLED_SKILLS_13",
    "BotHomeFixture",
    "CLI_BIN",
    "CLOSED_TAB_TOMBSTONE_DAYS",
    "CliResult",
    "E2EE_V2_KEY_SCHEDULE_BYTES",
    "GitRepoFixture",
    "HttpResponse",
    "IDE_ERROR_CODES",
    "LOOK_AROUND_AGENTS",
    "LOOK_AROUND_MAX_SUGGESTIONS",
    "MIN_COMPATIBLE_PROTOCOL_VERSION",
    "MockMcpUpstreamServer",
    "MockOAuthServer",
    "MockWebhookSink",
    "ORCA_YAML_MAX_ALIAS_COUNT",
    "ORCA_YAML_MAX_BYTES",
    "ORCA_YAML_MAX_DEFAULT_TABS",
    "ORCA_YAML_MAX_SHARED_DIRS",
    "ORCHESTRATION_MESSAGE_TYPES_9",
    "OrcaApiClient",
    "OrcaCliRunner",
    "OrcaServerHandle",
    "PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC",
    "PLUGIN_APPROVAL_REQUIRED_EXIT_CODE",
    "PLUGIN_TIERS_4",
    "PYTEST_BIN",
    "READD_GUARD_DAYS",
    "REPO_ROOT",
    "RUNTIME_PROTOCOL_VERSION",
    "RawHttpClient",
    "RecordedRequest",
    "SERVER_BIN",
    "SHARED_MCP_FAILED_START_DEBOUNCE_MS",
    "SUITE_ROOT",
    "SqliteFixtureHelper",
    "TAB_CONTENT_TYPES_10",
    "TARGET_RELEASE_TAG",
    "TARGET_RELEASE_TITLE",
    "TARGET_RELEASE_VERSION",
    "TEMPLATE_CATEGORIES_6",
    "TEMPLATE_IDS_24",
    "TIDY_UP_AGENTS",
    "TIDY_UP_MAX_FILED",
    "TIDY_UP_MAX_NEW_FACTS",
    "TIDY_UP_TARGET_FACTS",
    "TRIGGER_SOURCES_6_KINDS_31",
    "TempWorkspace",
    "USAGE_PROVIDERS_9",
    "WAIT_DEFAULT_EXPIRY_DAYS",
    "WAIT_MAX_DEADLINE_DAYS",
    "WATCH_PR_MAX_FIX_ATTEMPTS",
    "WEBHOOK_ACCOUNT_HOURLY_CAP",
    "WEBHOOK_ADAPTERS_6",
    "WEBHOOK_CALL_STATES_COUNT",
    "WEBHOOK_FILTER_OPS_6",
    "WEBHOOK_HOOK_HOURLY_CAP",
    "WEBHOOK_MAX_PICK",
    "WEBHOOK_MAX_RULE_BYTES",
    "WEBHOOK_MAX_VALUES",
    "WEBHOOK_MAX_WHERE",
    "WORKTREE_INCLUDE_MAX_BYTES",
    "WORKTREE_INCLUDE_MAX_ENTRIES",
    "WORKTREE_INCLUDE_MAX_PATHS",
    "WS_CLOSE_MESSAGE_TOO_BIG",
    "WS_CLOSE_SUPERSEDED",
    "WsClient",
    "WsFrame",
    "allocate_free_port",
    "pkce_s256_challenge",
]
