//! Complete SQLite `openbot.db` Schema `v69` DDL (`197` non-`schema_migrations` `sqlite_master`
//! declarations: `94` regular tables + `agent_memory_search` FTS5 `unicode61` virtual table +
//! `4` FTS5 shadow tables + `95` explicit indexes + `3` `agent_memory_*` triggers), contiguous
//! `v8`..`v69` migration validation, built-in `crm_statuses` seed rows, `PRAGMA foreign_key_check`
//! `(table -> parent)` delta guard, and `PRAGMA quick_check(20)` integrity verification.
//!
//! Mirrors `src/backend/openbot-database-schema.ts` and `src/backend/migrations/0008..0069`.

use std::collections::{BTreeMap, HashSet};

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};

use crate::error::{StorageError, StorageResult};

pub const BASELINE_SCHEMA_VERSION: i64 = 8;
pub const LATEST_SCHEMA_VERSION: i64 = 69;

/// Section 1 of dependency-ordered `v69` SQLite DDL (`[00]`..`[30]`).
const DDL_PART_1: &str = r#"
-- [00] schema_migrations
CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);

-- [01] agent_usage_activity
CREATE TABLE IF NOT EXISTS agent_usage_activity(agent_id TEXT NOT NULL, activity_id TEXT NOT NULL, session_id TEXT NOT NULL, turn_id TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('turn', 'user', 'assistant')), occurred_at TEXT NOT NULL, PRIMARY KEY(agent_id, activity_id));

-- [02] agent_usage_checkpoints
CREATE TABLE IF NOT EXISTS agent_usage_checkpoints(agent_id TEXT NOT NULL, counter_id TEXT NOT NULL, tokens_json TEXT NOT NULL CHECK(json_valid(tokens_json)), PRIMARY KEY(agent_id, counter_id));

-- [03] agent_usage_records
CREATE TABLE IF NOT EXISTS agent_usage_records(agent_id TEXT NOT NULL, record_id TEXT NOT NULL, session_id TEXT NOT NULL, turn_id TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, occurred_at TEXT NOT NULL, tokens_json TEXT NOT NULL CHECK(json_valid(tokens_json)), estimated_cost_usd REAL, rate_basis TEXT, recorded_at TEXT NOT NULL, PRIMARY KEY(agent_id, record_id));

-- [04] agent_usage_skill_loads
CREATE TABLE IF NOT EXISTS agent_usage_skill_loads(agent_id TEXT NOT NULL, load_id TEXT NOT NULL, session_id TEXT NOT NULL, turn_id TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, skill TEXT NOT NULL CHECK(length(skill) BETWEEN 1 AND 128), tokens INTEGER CHECK(tokens IS NULL OR tokens >= 0), estimated_cost_usd REAL CHECK(estimated_cost_usd IS NULL OR estimated_cost_usd >= 0), occurred_at TEXT NOT NULL, PRIMARY KEY(agent_id, load_id));

-- [05] browser_profiles
CREATE TABLE IF NOT EXISTS browser_profiles(profile_id TEXT NOT NULL PRIMARY KEY CHECK(length(profile_id) = 32), name TEXT NOT NULL, hue INTEGER NOT NULL CHECK(hue IN (0, 30, 55, 100, 150, 185, 215, 245, 280, 320)), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [06] browser_proxies
CREATE TABLE IF NOT EXISTS browser_proxies(proxy_id TEXT NOT NULL PRIMARY KEY CHECK(length(proxy_id) = 32), kind TEXT NOT NULL DEFAULT 'address' CHECK(kind IN ('address', 'computer')), scheme TEXT CHECK(scheme IN ('socks5', 'http', 'https')), host TEXT CHECK(length(host) BETWEEN 1 AND 255), port INTEGER CHECK(port BETWEEN 1 AND 65535), username TEXT, password BLOB, member_id TEXT CHECK(length(member_id) BETWEEN 1 AND 255), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, CHECK(password IS NULL OR username IS NOT NULL), CHECK((kind = 'address' AND scheme IS NOT NULL AND host IS NOT NULL AND port IS NOT NULL AND member_id IS NULL) OR (kind = 'computer' AND member_id IS NOT NULL AND scheme IS NULL AND host IS NULL AND port IS NULL AND username IS NULL AND password IS NULL)));

-- [07] crm_agent_access
CREATE TABLE IF NOT EXISTS crm_agent_access(agent_id TEXT NOT NULL PRIMARY KEY CHECK(length(agent_id) BETWEEN 1 AND 255), granted_at TEXT NOT NULL);

-- [08] crm_counters
CREATE TABLE IF NOT EXISTS crm_counters(name TEXT NOT NULL PRIMARY KEY CHECK(name IN ('customer', 'payment')), value INTEGER NOT NULL CHECK(value >= 0));

-- [09] crm_private_fields
CREATE TABLE IF NOT EXISTS crm_private_fields(field TEXT NOT NULL PRIMARY KEY CHECK(field IN ('name', 'contactName', 'contactEmail', 'phone', 'billingEmail', 'domain', 'taxId', 'address', 'stripeCustomerId', 'notes', 'amounts', 'dates', 'status', 'tags', 'plans', 'method', 'owner')));

-- [10] crm_source_ignored
CREATE TABLE IF NOT EXISTS crm_source_ignored(source_ref TEXT NOT NULL PRIMARY KEY CHECK(length(source_ref) BETWEEN 1 AND 255)) WITHOUT ROWID;

-- [11] crm_sources
CREATE TABLE IF NOT EXISTS crm_sources(source_id TEXT NOT NULL PRIMARY KEY CHECK(length(source_id) = 32), kind TEXT NOT NULL UNIQUE CHECK(kind IN ('stripe')), test_mode INTEGER NOT NULL CHECK(test_mode IN (0, 1)), account_label TEXT NOT NULL DEFAULT '' CHECK(length(account_label) <= 200), key_hint TEXT NOT NULL CHECK(length(key_hint) <= 8), state TEXT NOT NULL CHECK(state IN ('syncing', 'ok', 'error')), error_code TEXT CHECK(error_code IS NULL OR error_code IN ('invalid-key', 'permission', 'rate-limited', 'network', 'stripe')), synced_at TEXT, skipped_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(skipped_json)), connected_at TEXT NOT NULL);

-- [12] crm_statuses
CREATE TABLE IF NOT EXISTS crm_statuses(status_id TEXT NOT NULL PRIMARY KEY CHECK(length(status_id) = 32 OR status_id IN ('lead', 'customer', 'former')), label TEXT COLLATE NOCASE UNIQUE CHECK(label IS NULL OR length(label) BETWEEN 1 AND 80), tone TEXT NOT NULL CHECK(tone IN ('info-light', 'secondary', 'outline', 'warning-light', 'success-light', 'primary-light')), position INTEGER NOT NULL CHECK(position >= 0), created_at TEXT NOT NULL, CHECK((status_id IN ('lead', 'customer', 'former')) = (label IS NULL)));

-- [13] crm_tags
CREATE TABLE IF NOT EXISTS crm_tags(tag_id TEXT NOT NULL PRIMARY KEY CHECK(length(tag_id) = 32), label TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK(length(label) BETWEEN 1 AND 40), created_at TEXT NOT NULL);

-- [14] file_deletion_outbox
CREATE TABLE IF NOT EXISTS file_deletion_outbox(id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, reason TEXT NOT NULL, created_at TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT);

-- [15] orchestration_command_receipts
CREATE TABLE IF NOT EXISTS orchestration_command_receipts(command_id TEXT PRIMARY KEY, accepted_at TEXT NOT NULL, first_sequence INTEGER NOT NULL, last_sequence INTEGER NOT NULL, result_json TEXT NOT NULL CHECK(json_valid(result_json)));

-- [16] orchestration_events
CREATE TABLE IF NOT EXISTS orchestration_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT NOT NULL UNIQUE, command_id TEXT NOT NULL, aggregate_type TEXT NOT NULL, aggregate_id TEXT NOT NULL, event_type TEXT NOT NULL, occurred_at TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)));

-- [17] projection_agent_memories
CREATE TABLE IF NOT EXISTS projection_agent_memories(memory_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, text TEXT NOT NULL, normalized_text TEXT NOT NULL, origin TEXT NOT NULL CHECK(origin IN ('automatic', 'manual')), source_turn_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, site TEXT CHECK(site IS NULL OR length(site) BETWEEN 1 AND 253), UNIQUE(agent_id, normalized_text));

-- [18] projection_agent_routines
CREATE TABLE IF NOT EXISTS projection_agent_routines(routine_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, name TEXT NOT NULL, instruction TEXT NOT NULL, active INTEGER NOT NULL CHECK(active IN (0, 1)), timezone TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, limit_policy TEXT NOT NULL DEFAULT 'wait' CHECK(limit_policy IN ('wait', 'skip')), missed_policy TEXT NOT NULL DEFAULT 'skip' CHECK(missed_policy IN ('skip', 'run-once')), run_in TEXT NOT NULL DEFAULT 'chat' CHECK(run_in IN ('chat', 'thread')));

-- [19] projection_agents
CREATE TABLE IF NOT EXISTS projection_agents(agent_id TEXT PRIMARY KEY, thread_id TEXT, model TEXT NOT NULL, updated_at TEXT, sort_order INTEGER NOT NULL, agent_json TEXT NOT NULL CHECK(json_valid(agent_json)), last_event_sequence INTEGER NOT NULL);

-- [20] projection_attachments
CREATE TABLE IF NOT EXISTS projection_attachments(attachment_id TEXT PRIMARY KEY, owner_kind TEXT NOT NULL, owner_id TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL, metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)), created_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL);

-- [21] projection_channels
CREATE TABLE IF NOT EXISTS projection_channels(channel_id TEXT PRIMARY KEY, channel_json TEXT NOT NULL CHECK(json_valid(channel_json)));

-- [22] projection_direct_threads
CREATE TABLE IF NOT EXISTS projection_direct_threads(thread_id TEXT PRIMARY KEY, member_a_id TEXT NOT NULL, member_b_id TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_message_id TEXT, last_event_sequence INTEGER NOT NULL, UNIQUE(member_a_id, member_b_id));

-- [23] projection_mailbox_messages
CREATE TABLE IF NOT EXISTS projection_mailbox_messages(message_id TEXT PRIMARY KEY, sender_kind TEXT NOT NULL, sender_agent_id TEXT, text TEXT NOT NULL, reply_to_message_id TEXT, created_at TEXT NOT NULL, message_json TEXT NOT NULL CHECK(json_valid(message_json)), last_event_sequence INTEGER NOT NULL);

-- [24] projection_mcp_servers
CREATE TABLE IF NOT EXISTS projection_mcp_servers(mcp_server_id TEXT PRIMARY KEY, name TEXT NOT NULL, transport TEXT NOT NULL CHECK(transport IN ('stdio', 'http')), enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)), command TEXT NOT NULL, args_json TEXT NOT NULL CHECK(json_valid(args_json)), env_json TEXT NOT NULL CHECK(json_valid(env_json)), env_passthrough_json TEXT NOT NULL CHECK(json_valid(env_passthrough_json)), working_directory TEXT NOT NULL, url TEXT NOT NULL, headers_json TEXT NOT NULL CHECK(json_valid(headers_json)), position INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [25] projection_messaging_connections
CREATE TABLE IF NOT EXISTS projection_messaging_connections(connection_id TEXT PRIMARY KEY, platform TEXT NOT NULL, workspace_id TEXT NOT NULL, workspace_name TEXT NOT NULL, enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)), bot_user_id TEXT, app_id TEXT, orchestrator_agent_id TEXT, last_error_code TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(platform, workspace_id));

-- [26] projection_messaging_history
CREATE TABLE IF NOT EXISTS projection_messaging_history(seq INTEGER PRIMARY KEY, connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), platform_channel_id TEXT NOT NULL, message_id TEXT NOT NULL, thread_key TEXT NOT NULL, author_id TEXT NOT NULL, author_name TEXT NOT NULL, text TEXT NOT NULL, sent_at TEXT NOT NULL, files_json TEXT NOT NULL, saved_at TEXT NOT NULL, UNIQUE(connection_id, platform_channel_id, message_id));

-- [27] projection_messaging_memories
CREATE TABLE IF NOT EXISTS projection_messaging_memories(memory_id TEXT PRIMARY KEY, connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), text TEXT NOT NULL, normalized_text TEXT NOT NULL, origin TEXT NOT NULL CHECK(origin IN ('automatic', 'manual')), source_turn_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, UNIQUE(connection_id, normalized_text));

-- [28] projection_messaging_pings
CREATE TABLE IF NOT EXISTS projection_messaging_pings(connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), platform_channel_id TEXT NOT NULL, thread_key TEXT NOT NULL, user_id TEXT NOT NULL, agent_id TEXT NOT NULL, text TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(connection_id, platform_channel_id, thread_key, user_id));

-- [29] projection_queue_state
CREATE TABLE IF NOT EXISTS projection_queue_state(agent_id TEXT PRIMARY KEY, paused INTEGER NOT NULL CHECK(paused IN (0, 1)), metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)), last_event_sequence INTEGER NOT NULL);

-- [30] projection_reactions
CREATE TABLE IF NOT EXISTS projection_reactions(agent_id TEXT NOT NULL, message_id TEXT NOT NULL, emoji TEXT NOT NULL, actor_kind TEXT NOT NULL CHECK(actor_kind IN ('user', 'agent')), actor_agent_id TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, PRIMARY KEY(agent_id, message_id, actor_kind, actor_agent_id));
"#;

/// Section 2 of dependency-ordered `v69` SQLite DDL (`[31]`..`[65]`).
const DDL_PART_2: &str = r#"
-- [31] projection_routine_runs
CREATE TABLE IF NOT EXISTS projection_routine_runs(run_id TEXT PRIMARY KEY, routine_id TEXT NOT NULL REFERENCES projection_agent_routines(routine_id) ON DELETE CASCADE, agent_id TEXT NOT NULL, trigger_id TEXT, run_kind TEXT NOT NULL CHECK(run_kind IN ('scheduled', 'manual')), scheduled_for TEXT NOT NULL, routine_name TEXT NOT NULL, instruction TEXT NOT NULL, delivery_id TEXT, status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'needs-attention', 'succeeded', 'failed', 'interrupted', 'cancelled')), error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, missed_count INTEGER CHECK(missed_count IS NULL OR missed_count >= 1), missed_until TEXT, run_source TEXT CHECK(run_source IS NULL OR run_source IN ('test', 'script', 'webhook')), UNIQUE(trigger_id, scheduled_for));

-- [32] projection_routine_triggers
CREATE TABLE IF NOT EXISTS projection_routine_triggers(trigger_id TEXT PRIMARY KEY, routine_id TEXT NOT NULL REFERENCES projection_agent_routines(routine_id) ON DELETE CASCADE, schedule_json TEXT NOT NULL CHECK(json_valid(schedule_json)), next_run_at TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, UNIQUE(routine_id));

-- [33] projection_routine_webhooks
CREATE TABLE IF NOT EXISTS projection_routine_webhooks(routine_id TEXT PRIMARY KEY REFERENCES projection_agent_routines(routine_id) ON DELETE CASCADE, route_id TEXT NOT NULL UNIQUE, event_type TEXT, filters_json TEXT NOT NULL CHECK(json_valid(filters_json)), secret_ciphertext TEXT NOT NULL, url TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL);

-- [34] projection_threads
CREATE TABLE IF NOT EXISTS projection_threads(thread_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, title TEXT NOT NULL, active_turn_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL);

-- [35] projection_webhook_receipts
CREATE TABLE IF NOT EXISTS projection_webhook_receipts(receipt_id TEXT PRIMARY KEY, owner_kind TEXT NOT NULL CHECK(owner_kind IN ('agent', 'channel')), routine_id TEXT NOT NULL, delivery_id TEXT NOT NULL, event_type TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('started', 'ignored')), reason TEXT CHECK(reason IS NULL OR reason IN ('event-type', 'filter', 'inactive')), run_id TEXT, received_at TEXT NOT NULL, UNIQUE(owner_kind, routine_id, delivery_id));

-- [36] projection_webhook_route_revocations
CREATE TABLE IF NOT EXISTS projection_webhook_route_revocations(route_id TEXT PRIMARY KEY, created_at TEXT NOT NULL);

-- [37] provider_history_imports
CREATE TABLE IF NOT EXISTS provider_history_imports(session_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, provider TEXT NOT NULL, external_session_id TEXT NOT NULL, cursor_json TEXT CHECK(cursor_json IS NULL OR json_valid(cursor_json)), state TEXT NOT NULL CHECK(state IN ('pending', 'active', 'complete', 'failed')), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(provider, external_session_id));

-- [38] provider_history_staging
CREATE TABLE IF NOT EXISTS provider_history_staging(session_id TEXT NOT NULL REFERENCES provider_history_imports(session_id) ON DELETE CASCADE, turn_id TEXT NOT NULL, item_key TEXT NOT NULL, item_index INTEGER NOT NULL, item_json TEXT NOT NULL CHECK(json_valid(item_json)), imported INTEGER NOT NULL DEFAULT 0 CHECK(imported IN (0, 1)), updated_at TEXT NOT NULL, PRIMARY KEY(session_id, turn_id, item_key));

-- [39] provider_history_turns
CREATE TABLE IF NOT EXISTS provider_history_turns(session_id TEXT NOT NULL REFERENCES provider_history_imports(session_id) ON DELETE CASCADE, turn_id TEXT NOT NULL, turn_status TEXT, turn_started_at INTEGER, complete INTEGER NOT NULL CHECK(complete IN (0, 1)), imported INTEGER NOT NULL DEFAULT 0 CHECK(imported IN (0, 1)), updated_at TEXT NOT NULL, PRIMARY KEY(session_id, turn_id));

-- [40] routine_flow_links
CREATE TABLE IF NOT EXISTS routine_flow_links(link_id TEXT PRIMARY KEY, routine_id TEXT NOT NULL REFERENCES projection_agent_routines(routine_id) ON DELETE CASCADE, from_agent_id TEXT NOT NULL, to_agent_id TEXT NOT NULL, instruction TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, CHECK(from_agent_id <> to_agent_id), UNIQUE(routine_id, from_agent_id, to_agent_id));

-- [41] routine_flow_positions
CREATE TABLE IF NOT EXISTS routine_flow_positions(canvas_agent_id TEXT NOT NULL, node_key TEXT NOT NULL, x REAL NOT NULL, y REAL NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(canvas_agent_id, node_key));

-- [42] routine_flow_steps
CREATE TABLE IF NOT EXISTS routine_flow_steps(step_id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES projection_routine_runs(run_id) ON DELETE CASCADE, agent_id TEXT NOT NULL, delivery_id TEXT, input TEXT NOT NULL, output TEXT, status TEXT NOT NULL CHECK(status IN ('running', 'succeeded', 'failed', 'skipped', 'cancelled')), error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(run_id, agent_id));

-- [43] routine_result_deliveries
CREATE TABLE IF NOT EXISTS routine_result_deliveries(run_id TEXT PRIMARY KEY REFERENCES projection_routine_runs(run_id) ON DELETE CASCADE, routine_id TEXT NOT NULL, agent_id TEXT NOT NULL, connection_id TEXT NOT NULL, platform TEXT NOT NULL, connection_name TEXT NOT NULL, place_id TEXT NOT NULL, place_name TEXT NOT NULL, text TEXT, status TEXT NOT NULL CHECK(status IN ('pending', 'sending', 'sent', 'failed', 'uncertain', 'quiet')), reason TEXT, attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0), next_attempt_at TEXT, expires_at TEXT, sent_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [44] routine_result_destinations
CREATE TABLE IF NOT EXISTS routine_result_destinations(routine_id TEXT PRIMARY KEY REFERENCES projection_agent_routines(routine_id) ON DELETE CASCADE, connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), platform TEXT NOT NULL, connection_name TEXT NOT NULL, place_id TEXT NOT NULL, place_name TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [45] thread_event_compaction
CREATE TABLE IF NOT EXISTS thread_event_compaction(thread_id TEXT PRIMARY KEY REFERENCES projection_threads(thread_id) ON DELETE CASCADE, through_sequence INTEGER NOT NULL CHECK(through_sequence >= 0), compacted_at TEXT NOT NULL);

-- [46] workflows
CREATE TABLE IF NOT EXISTS workflows(workflow_id TEXT PRIMARY KEY, title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 160), graph_revision INTEGER NOT NULL DEFAULT 0 CHECK(graph_revision >= 0), strokes_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(strokes_json)), next_run_number INTEGER NOT NULL DEFAULT 1 CHECK(next_run_number >= 1), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, paused INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0, 1)));

-- [47] agent_memory_search (FTS5 Virtual Table)
CREATE VIRTUAL TABLE IF NOT EXISTS agent_memory_search USING FTS5(text, content = 'projection_agent_memories', content_rowid = 'rowid', tokenize = 'unicode61');

-- [48] agent_memory_selections
CREATE TABLE IF NOT EXISTS agent_memory_selections(memory_id TEXT PRIMARY KEY REFERENCES projection_agent_memories(memory_id) ON DELETE CASCADE, inclusion TEXT NOT NULL DEFAULT 'searchable' CHECK(inclusion IN ('essential', 'searchable')), user_controlled INTEGER NOT NULL DEFAULT 0 CHECK(user_controlled IN (0, 1)), revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0));

-- [49] agent_waits
CREATE TABLE IF NOT EXISTS agent_waits(wait_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, request_delivery_id TEXT NOT NULL, request_text TEXT NOT NULL, note TEXT NOT NULL, routine_id TEXT REFERENCES projection_agent_routines(routine_id) ON DELETE SET NULL, created_at TEXT NOT NULL, woken_at TEXT, wake_delivery_id TEXT);

-- [50] browser_profile_proxies
CREATE TABLE IF NOT EXISTS browser_profile_proxies(profile_id TEXT NOT NULL PRIMARY KEY REFERENCES browser_profiles(profile_id) ON DELETE CASCADE, proxy_id TEXT NOT NULL REFERENCES browser_proxies(proxy_id) ON DELETE CASCADE);

-- [51] crm_customers
CREATE TABLE IF NOT EXISTS crm_customers(customer_id TEXT NOT NULL PRIMARY KEY CHECK(length(customer_id) = 32), number INTEGER NOT NULL UNIQUE CHECK(number >= 1), name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 200), status_id TEXT NOT NULL REFERENCES crm_statuses(status_id), owner_agent_id TEXT CHECK(owner_agent_id IS NULL OR length(owner_agent_id) BETWEEN 1 AND 255), method TEXT CHECK(method IS NULL OR method IN ('stripe', 'transfer', 'crypto', 'paypal', 'cash')), currency TEXT NOT NULL CHECK(length(currency) = 3 AND currency GLOB '[A-Z][A-Z][A-Z]'), contact_name TEXT NOT NULL DEFAULT '' CHECK(length(contact_name) <= 500), contact_email TEXT NOT NULL DEFAULT '' CHECK(length(contact_email) <= 500), phone TEXT NOT NULL DEFAULT '' CHECK(length(phone) <= 500), billing_email TEXT NOT NULL DEFAULT '' CHECK(length(billing_email) <= 500), domain TEXT NOT NULL DEFAULT '' CHECK(length(domain) <= 500), tax_id TEXT NOT NULL DEFAULT '' CHECK(length(tax_id) <= 500), address TEXT NOT NULL DEFAULT '' CHECK(length(address) <= 500), stripe_customer_id TEXT NOT NULL DEFAULT '' CHECK(length(stripe_customer_id) <= 500), notes TEXT NOT NULL DEFAULT '' CHECK(length(notes) <= 4000), search_text TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(search_text)), source_id TEXT REFERENCES crm_sources(source_id) ON DELETE SET NULL, source_ref TEXT CHECK(source_ref IS NULL OR length(source_ref) BETWEEN 1 AND 255), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1 CHECK(revision >= 1));

-- [52] crm_plans
CREATE TABLE IF NOT EXISTS crm_plans(plan_id TEXT NOT NULL PRIMARY KEY CHECK(length(plan_id) = 32), customer_id TEXT NOT NULL REFERENCES crm_customers(customer_id) ON DELETE CASCADE, name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80), amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 0 AND 1000000000000), every TEXT NOT NULL CHECK(every IN ('month', 'quarter', 'year', 'once')), starts_on TEXT NOT NULL CHECK(starts_on GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'), ends_on TEXT CHECK(ends_on IS NULL OR (ends_on GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' AND ends_on >= starts_on)), source_id TEXT REFERENCES crm_sources(source_id) ON DELETE SET NULL, source_ref TEXT CHECK(source_ref IS NULL OR length(source_ref) BETWEEN 1 AND 255), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [53] crm_proposals
CREATE TABLE IF NOT EXISTS crm_proposals(proposal_id TEXT NOT NULL PRIMARY KEY CHECK(length(proposal_id) = 32), agent_id TEXT NOT NULL CHECK(length(agent_id) BETWEEN 1 AND 255), customer_id TEXT REFERENCES crm_customers(customer_id) ON DELETE CASCADE, thread_id TEXT CHECK(thread_id IS NULL OR length(thread_id) BETWEEN 1 AND 255), reason TEXT NOT NULL CHECK(length(reason) <= 1000), change_json TEXT NOT NULL CHECK(json_valid(change_json)), state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending', 'approved', 'rejected')), created_at TEXT NOT NULL, decided_at TEXT, decided_by_id TEXT, decided_by_name TEXT, CHECK((state = 'pending') = (decided_at IS NULL)));

-- [54] projection_agent_threads
CREATE TABLE IF NOT EXISTS projection_agent_threads(thread_id TEXT PRIMARY KEY REFERENCES projection_threads(thread_id) ON DELETE CASCADE, agent_id TEXT NOT NULL, legacy_task_id TEXT UNIQUE, origin TEXT NOT NULL CHECK(origin IN ('user', 'agent')), title TEXT NOT NULL, title_source TEXT NOT NULL CHECK(title_source IN ('auto', 'agent', 'user')), outcome TEXT CHECK(outcome IS NULL OR outcome IN ('completed', 'failed', 'interrupted')), error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, archived_at TEXT, last_event_sequence INTEGER NOT NULL, branch_source_thread_id TEXT, branch_source_message_id TEXT, branch_through_message_id TEXT, routine_id TEXT);

-- [55] projection_channel_contexts
CREATE TABLE IF NOT EXISTS projection_channel_contexts(channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id), agent_id TEXT NOT NULL, thread_id TEXT NOT NULL UNIQUE REFERENCES projection_threads(thread_id), session_id TEXT, through_sequence INTEGER NOT NULL DEFAULT 0, summary_version INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(channel_id, agent_id));

-- [56] projection_channel_memories
CREATE TABLE IF NOT EXISTS projection_channel_memories(memory_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id) ON DELETE CASCADE, text TEXT NOT NULL, normalized_text TEXT NOT NULL, origin TEXT NOT NULL CHECK(origin IN ('automatic', 'manual')), source_turn_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, UNIQUE(channel_id, normalized_text));

-- [57] projection_channel_messages
CREATE TABLE IF NOT EXISTS projection_channel_messages(channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id), message_id TEXT NOT NULL, sequence INTEGER NOT NULL, message_json TEXT NOT NULL CHECK(json_valid(message_json)), PRIMARY KEY(channel_id, message_id));

-- [58] projection_channel_reads
CREATE TABLE IF NOT EXISTS projection_channel_reads(channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id), member_id TEXT NOT NULL, through_sequence INTEGER NOT NULL, PRIMARY KEY(channel_id, member_id));

-- [59] projection_channel_routines
CREATE TABLE IF NOT EXISTS projection_channel_routines(routine_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id) ON DELETE CASCADE, name TEXT NOT NULL, instruction TEXT NOT NULL, active INTEGER NOT NULL CHECK(active IN (0, 1)), timezone TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, limit_policy TEXT NOT NULL DEFAULT 'wait' CHECK(limit_policy IN ('wait', 'skip')), missed_policy TEXT NOT NULL DEFAULT 'skip' CHECK(missed_policy IN ('skip', 'run-once')));

-- [60] projection_channel_summaries
CREATE TABLE IF NOT EXISTS projection_channel_summaries(channel_id TEXT PRIMARY KEY REFERENCES projection_channels(channel_id), version INTEGER NOT NULL, through_sequence INTEGER NOT NULL, text TEXT NOT NULL);

-- [61] projection_channel_tasks
CREATE TABLE IF NOT EXISTS projection_channel_tasks(task_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id), task_json TEXT NOT NULL CHECK(json_valid(task_json)));

-- [62] projection_deleted_thread_messages
CREATE TABLE IF NOT EXISTS projection_deleted_thread_messages(thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, message_id TEXT NOT NULL CHECK(length(message_id) > 0), deleted_at TEXT NOT NULL, PRIMARY KEY(thread_id, message_id));

-- [63] projection_deliveries
CREATE TABLE IF NOT EXISTS projection_deliveries(delivery_id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES projection_mailbox_messages(message_id) ON DELETE CASCADE, recipient_agent_id TEXT NOT NULL, status TEXT NOT NULL, turn_id TEXT, error TEXT, created_at TEXT NOT NULL, delivery_json TEXT NOT NULL CHECK(json_valid(delivery_json)), last_event_sequence INTEGER NOT NULL);

-- [64] projection_direct_messages
CREATE TABLE IF NOT EXISTS projection_direct_messages(message_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES projection_direct_threads(thread_id) ON DELETE CASCADE, sender_member_id TEXT NOT NULL, recipient_member_id TEXT NOT NULL, text TEXT NOT NULL, created_at TEXT NOT NULL, message_json TEXT NOT NULL CHECK(json_valid(message_json)), last_event_sequence INTEGER NOT NULL);

-- [65] projection_direct_reads
CREATE TABLE IF NOT EXISTS projection_direct_reads(thread_id TEXT NOT NULL REFERENCES projection_direct_threads(thread_id) ON DELETE CASCADE, member_id TEXT NOT NULL, last_read_sequence INTEGER NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(thread_id, member_id));
"#;

/// Section 3 of dependency-ordered `v69` SQLite DDL (`[66]`..`[95]`).
const DDL_PART_3: &str = r#"
-- [66] projection_messaging_allowed_roles
CREATE TABLE IF NOT EXISTS projection_messaging_allowed_roles(connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), role_id TEXT NOT NULL, role_name TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(connection_id, role_id));

-- [67] projection_messaging_allowed_senders
CREATE TABLE IF NOT EXISTS projection_messaging_allowed_senders(connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), sender_id TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('contact', 'group')), name TEXT NOT NULL, created_at TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'reply' CHECK(mode IN ('reply', 'read')), PRIMARY KEY(connection_id, sender_id));

-- [68] projection_messaging_threads
CREATE TABLE IF NOT EXISTS projection_messaging_threads(link_id TEXT PRIMARY KEY, connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), agent_id TEXT NOT NULL, platform_channel_id TEXT NOT NULL, thread_key TEXT NOT NULL, is_direct INTEGER NOT NULL CHECK(is_direct IN (0, 1)), thread_id TEXT NOT NULL UNIQUE REFERENCES projection_threads(thread_id), title TEXT NOT NULL, history_cursor TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(connection_id, platform_channel_id, thread_key));

-- [69] projection_pinned_thread_messages
CREATE TABLE IF NOT EXISTS projection_pinned_thread_messages(thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, message_id TEXT NOT NULL CHECK(length(message_id) > 0), pinned_at TEXT NOT NULL, pinned_by_kind TEXT NOT NULL CHECK(pinned_by_kind IN ('person', 'agent')), pinned_by_id TEXT CHECK(pinned_by_id IS NULL OR length(pinned_by_id) > 0), pinned_by_name TEXT, PRIMARY KEY(thread_id, message_id), CHECK((pinned_by_kind = 'agent' AND pinned_by_id IS NOT NULL AND pinned_by_name IS NULL) OR (pinned_by_kind = 'person' AND (pinned_by_id IS NULL) = (pinned_by_name IS NULL))));

-- [70] projection_provider_sessions
CREATE TABLE IF NOT EXISTS projection_provider_sessions(id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, provider TEXT NOT NULL, external_session_id TEXT NOT NULL, model TEXT NOT NULL, effort TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('active', 'inactive', 'failed')), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, resume_cursor TEXT, last_event_sequence INTEGER NOT NULL, UNIQUE(provider, external_session_id));

-- [71] projection_thread_activities
CREATE TABLE IF NOT EXISTS projection_thread_activities(activity_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, turn_id TEXT, activity_type TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)), created_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL);

-- [72] projection_thread_messages
CREATE TABLE IF NOT EXISTS projection_thread_messages(thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, message_id TEXT NOT NULL, turn_id TEXT, author TEXT NOT NULL, status TEXT NOT NULL, item_type TEXT, created_at TEXT NOT NULL, ordinal INTEGER NOT NULL, message_json TEXT NOT NULL CHECK(json_valid(message_json)), last_event_sequence INTEGER NOT NULL, group_start TEXT, group_first INTEGER, group_id TEXT, turn_rank INTEGER, PRIMARY KEY(thread_id, message_id));

-- [73] projection_thread_read_baselines
CREATE TABLE IF NOT EXISTS projection_thread_read_baselines(thread_id TEXT PRIMARY KEY REFERENCES projection_threads(thread_id) ON DELETE CASCADE, through_message_id TEXT, initialized_at TEXT NOT NULL);

-- [74] projection_thread_reads
CREATE TABLE IF NOT EXISTS projection_thread_reads(thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, member_id TEXT NOT NULL, through_message_id TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(thread_id, member_id));

-- [75] projection_thread_summaries
CREATE TABLE IF NOT EXISTS projection_thread_summaries(summary_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, through_message_id TEXT, summary_text TEXT NOT NULL, estimated_tokens INTEGER NOT NULL, created_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL);

-- [76] projection_turns
CREATE TABLE IF NOT EXISTS projection_turns(turn_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES projection_threads(thread_id) ON DELETE CASCADE, provider_session_id TEXT REFERENCES projection_provider_sessions(id) ON DELETE SET NULL, status TEXT NOT NULL, started_at TEXT NOT NULL, completed_at TEXT, last_event_sequence INTEGER NOT NULL);

-- [77] workflow_nodes
CREATE TABLE IF NOT EXISTS workflow_nodes(workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, node_id TEXT NOT NULL, position INTEGER NOT NULL CHECK(position >= 0), kind TEXT NOT NULL CHECK(kind IN ('text', 'prompt', 'asset', 'llm', 'agent', 'image', 'trigger', 'condition', 'approval', 'output', 'note', 'shape')), agent_id TEXT, x REAL NOT NULL, y REAL NOT NULL, width REAL, height REAL, font REAL, locked INTEGER NOT NULL DEFAULT 0 CHECK(locked IN (0, 1)), fields_json TEXT NOT NULL CHECK(json_valid(fields_json)), schedule_json TEXT CHECK(schedule_json IS NULL OR (kind = 'trigger' AND json_valid(schedule_json))), PRIMARY KEY(workflow_id, node_id));

-- [78] workflow_runs
CREATE TABLE IF NOT EXISTS workflow_runs(run_id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, number INTEGER NOT NULL CHECK(number >= 1), trigger_node_id TEXT, from_node_id TEXT, source TEXT NOT NULL CHECK(source IN ('manual', 'schedule', 'webhook', 'github', 'discord', 'telegram', 'slack')), scheduled_for TEXT, idempotency_key TEXT NOT NULL, graph_revision INTEGER NOT NULL CHECK(graph_revision >= 0), graph_json TEXT NOT NULL CHECK(json_valid(graph_json)), payload TEXT, status TEXT NOT NULL CHECK(status IN ('running', 'approval', 'stopping', 'done', 'failed', 'stopped')), error TEXT, started_at TEXT NOT NULL, ended_at TEXT, updated_at TEXT NOT NULL, UNIQUE(workflow_id, number), UNIQUE(workflow_id, idempotency_key));

-- [79] workflow_steps
CREATE TABLE IF NOT EXISTS workflow_steps(step_id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, run_id TEXT REFERENCES workflow_runs(run_id) ON DELETE CASCADE, node_id TEXT NOT NULL, attempt INTEGER NOT NULL CHECK(attempt >= 1), request_id TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('prompt', 'asset', 'llm', 'agent', 'image', 'trigger', 'condition', 'approval', 'output')), agent_id TEXT, status TEXT NOT NULL CHECK(status IN ('running', 'approval', 'done', 'failed', 'stopped', 'skipped')), delivery_id TEXT, turn_id TEXT, input TEXT NOT NULL DEFAULT '', output TEXT, branch TEXT CHECK(branch IS NULL OR branch IN ('yes', 'no')), error TEXT, started_at TEXT, ended_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, settings_json TEXT CHECK(settings_json IS NULL OR json_valid(settings_json)), sources_json TEXT CHECK(sources_json IS NULL OR json_valid(sources_json)), UNIQUE(workflow_id, request_id));

-- [80] workflow_triggers
CREATE TABLE IF NOT EXISTS workflow_triggers(workflow_id TEXT NOT NULL, node_id TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('manual', 'schedule', 'webhook')), timezone TEXT, schedule_json TEXT CHECK(schedule_json IS NULL OR json_valid(schedule_json)), next_due_at TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(workflow_id, node_id), FOREIGN KEY(workflow_id, node_id) REFERENCES workflow_nodes(workflow_id, node_id) ON DELETE CASCADE);

-- [81] workflow_versions
CREATE TABLE IF NOT EXISTS workflow_versions(version_id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, graph_revision INTEGER NOT NULL CHECK(graph_revision >= 0), kind TEXT NOT NULL CHECK(kind IN ('auto', 'restore', 'run')), graph_json TEXT NOT NULL CHECK(json_valid(graph_json)), created_at TEXT NOT NULL);

-- [82] workflow_webhook_receipts
CREATE TABLE IF NOT EXISTS workflow_webhook_receipts(receipt_id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, node_id TEXT NOT NULL, delivery_id TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('started', 'ignored')), reason TEXT CHECK(reason IS NULL OR reason IN ('busy', 'refused')), run_id TEXT, received_at TEXT NOT NULL, UNIQUE(workflow_id, node_id, delivery_id));

-- [83] crm_activity
CREATE TABLE IF NOT EXISTS crm_activity(activity_id TEXT NOT NULL PRIMARY KEY CHECK(length(activity_id) = 32), customer_id TEXT NOT NULL REFERENCES crm_customers(customer_id) ON DELETE CASCADE, actor_kind TEXT NOT NULL CHECK(actor_kind IN ('person', 'agent', 'source')), actor_id TEXT CHECK(actor_id IS NULL OR length(actor_id) BETWEEN 1 AND 255), actor_name TEXT CHECK(actor_name IS NULL OR length(actor_name) <= 200), kind TEXT NOT NULL CHECK(length(kind) BETWEEN 1 AND 48), details_json TEXT NOT NULL CHECK(json_valid(details_json)), proposed_by TEXT CHECK(proposed_by IS NULL OR length(proposed_by) BETWEEN 1 AND 255), created_at TEXT NOT NULL);

-- [84] crm_customer_tags
CREATE TABLE IF NOT EXISTS crm_customer_tags(customer_id TEXT NOT NULL REFERENCES crm_customers(customer_id) ON DELETE CASCADE, tag_id TEXT NOT NULL REFERENCES crm_tags(tag_id) ON DELETE CASCADE, PRIMARY KEY(customer_id, tag_id)) WITHOUT ROWID;

-- [85] crm_payments
CREATE TABLE IF NOT EXISTS crm_payments(payment_id TEXT NOT NULL PRIMARY KEY CHECK(length(payment_id) = 32), number INTEGER NOT NULL UNIQUE CHECK(number >= 1), customer_id TEXT NOT NULL REFERENCES crm_customers(customer_id) ON DELETE CASCADE, plan_id TEXT REFERENCES crm_plans(plan_id) ON DELETE SET NULL, label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 80), amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 0 AND 1000000000000), due_on TEXT NOT NULL CHECK(due_on GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'), paid_at TEXT CHECK(paid_at IS NULL OR length(paid_at) BETWEEN 1 AND 64), method TEXT CHECK(method IS NULL OR method IN ('stripe', 'transfer', 'crypto', 'paypal', 'cash')), source_id TEXT REFERENCES crm_sources(source_id) ON DELETE SET NULL, source_ref TEXT CHECK(source_ref IS NULL OR length(source_ref) BETWEEN 1 AND 255), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [86] projection_channel_assignments
CREATE TABLE IF NOT EXISTS projection_channel_assignments(assignment_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL REFERENCES projection_channels(channel_id), task_id TEXT NOT NULL REFERENCES projection_channel_tasks(task_id), delivery_id TEXT UNIQUE, assignment_json TEXT NOT NULL CHECK(json_valid(assignment_json)));

-- [87] projection_channel_routine_runs
CREATE TABLE IF NOT EXISTS projection_channel_routine_runs(run_id TEXT PRIMARY KEY, routine_id TEXT NOT NULL REFERENCES projection_channel_routines(routine_id) ON DELETE CASCADE, channel_id TEXT NOT NULL, trigger_id TEXT, run_kind TEXT NOT NULL CHECK(run_kind IN ('scheduled', 'manual')), scheduled_for TEXT NOT NULL, routine_name TEXT NOT NULL, instruction TEXT NOT NULL, request_message_id TEXT, status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'needs-attention', 'succeeded', 'failed', 'cancelled')), error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, missed_count INTEGER CHECK(missed_count IS NULL OR missed_count >= 1), missed_until TEXT, run_source TEXT CHECK(run_source IS NULL OR run_source IN ('test', 'script', 'webhook')), UNIQUE(trigger_id, scheduled_for));

-- [88] projection_channel_routine_triggers
CREATE TABLE IF NOT EXISTS projection_channel_routine_triggers(trigger_id TEXT PRIMARY KEY, routine_id TEXT NOT NULL REFERENCES projection_channel_routines(routine_id) ON DELETE CASCADE, schedule_json TEXT NOT NULL CHECK(json_valid(schedule_json)), next_run_at TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL, UNIQUE(routine_id));

-- [89] projection_channel_routine_webhooks
CREATE TABLE IF NOT EXISTS projection_channel_routine_webhooks(routine_id TEXT PRIMARY KEY REFERENCES projection_channel_routines(routine_id) ON DELETE CASCADE, route_id TEXT NOT NULL UNIQUE, event_type TEXT, filters_json TEXT NOT NULL CHECK(json_valid(filters_json)), secret_ciphertext TEXT NOT NULL, url TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_event_sequence INTEGER NOT NULL);

-- [90] workflow_edges
CREATE TABLE IF NOT EXISTS workflow_edges(workflow_id TEXT NOT NULL, edge_id TEXT NOT NULL, position INTEGER NOT NULL CHECK(position >= 0), from_node_id TEXT NOT NULL, to_node_id TEXT NOT NULL, port_type TEXT NOT NULL CHECK(port_type IN ('text', 'image', 'file')), branch TEXT CHECK(branch IS NULL OR branch IN ('yes', 'no')), CHECK(from_node_id <> to_node_id), PRIMARY KEY(workflow_id, edge_id), FOREIGN KEY(workflow_id, from_node_id) REFERENCES workflow_nodes(workflow_id, node_id) ON DELETE CASCADE, FOREIGN KEY(workflow_id, to_node_id) REFERENCES workflow_nodes(workflow_id, node_id) ON DELETE CASCADE);

-- [91] workflow_github_actions
CREATE TABLE IF NOT EXISTS workflow_github_actions(action_key TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, run_id TEXT REFERENCES workflow_runs(run_id) ON DELETE CASCADE, step_id TEXT NOT NULL, repository TEXT NOT NULL, number INTEGER CHECK(number IS NULL OR number >= 1), action TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('sending', 'done')), object_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [92] workflow_github_deliveries
CREATE TABLE IF NOT EXISTS workflow_github_deliveries(workflow_id TEXT NOT NULL, node_id TEXT NOT NULL, delivery_id TEXT NOT NULL, event TEXT NOT NULL, repository TEXT, number INTEGER CHECK(number IS NULL OR number >= 1), pull_request INTEGER CHECK(pull_request IS NULL OR pull_request IN (0, 1)), title TEXT, received_at TEXT NOT NULL, outcome TEXT NOT NULL CHECK(outcome IN ('started', 'waiting', 'skipped')), reason TEXT CHECK(reason IS NULL OR reason IN ('filter', 'self', 'chain', 'offline', 'refused')), run_id TEXT, payload TEXT, caused_by_bot INTEGER NOT NULL DEFAULT 0 CHECK(caused_by_bot IN (0, 1)), CHECK((repository IS NULL) = (number IS NULL) AND (number IS NULL) = (pull_request IS NULL) AND (number IS NULL) = (title IS NULL)), CHECK((outcome = 'skipped') = (reason IS NOT NULL)), CHECK(payload IS NULL OR outcome = 'waiting'), PRIMARY KEY(workflow_id, node_id, delivery_id), FOREIGN KEY(workflow_id, node_id) REFERENCES workflow_nodes(workflow_id, node_id) ON DELETE CASCADE);

-- [93] workflow_github_runs
CREATE TABLE IF NOT EXISTS workflow_github_runs(run_id TEXT PRIMARY KEY REFERENCES workflow_runs(run_id) ON DELETE CASCADE, workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE, node_id TEXT NOT NULL, delivery_id TEXT, repository TEXT NOT NULL, number INTEGER NOT NULL CHECK(number >= 1), pull_request INTEGER NOT NULL CHECK(pull_request IN (0, 1)), title TEXT NOT NULL, caused_by_bot INTEGER NOT NULL CHECK(caused_by_bot IN (0, 1)), started_at TEXT NOT NULL);

-- [94] workflow_output_deliveries
CREATE TABLE IF NOT EXISTS workflow_output_deliveries(step_id TEXT PRIMARY KEY REFERENCES workflow_steps(step_id) ON DELETE CASCADE, connection_id TEXT NOT NULL REFERENCES projection_messaging_connections(connection_id), place_id TEXT NOT NULL, text TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending', 'sending', 'sent', 'failed', 'uncertain')), reason TEXT, attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0), sent_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

-- [95] workflow_trigger_webhooks
CREATE TABLE IF NOT EXISTS workflow_trigger_webhooks(workflow_id TEXT NOT NULL, node_id TEXT NOT NULL, route_id TEXT NOT NULL UNIQUE, secret_ciphertext TEXT NOT NULL, url TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(workflow_id, node_id), FOREIGN KEY(workflow_id, node_id) REFERENCES workflow_triggers(workflow_id, node_id) ON DELETE CASCADE);
"#;

/// Section 4 of dependency-ordered `v69` SQLite DDL (`95` explicit indexes, `3` triggers, `3` CRM seeds).
const DDL_PART_4: &str = r#"
CREATE INDEX IF NOT EXISTS agent_memories_agent ON projection_agent_memories(agent_id, updated_at DESC, memory_id);
CREATE INDEX IF NOT EXISTS agent_memories_site ON projection_agent_memories(agent_id, site) WHERE site IS NOT NULL;
CREATE INDEX IF NOT EXISTS agent_routines_agent ON projection_agent_routines(agent_id, updated_at DESC, routine_id);
CREATE INDEX IF NOT EXISTS agent_threads_agent ON projection_agent_threads(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS agent_threads_open ON projection_agent_threads(agent_id, updated_at DESC) WHERE archived_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS agent_threads_routine ON projection_agent_threads(agent_id, routine_id) WHERE routine_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS agent_usage_activity_date ON agent_usage_activity(agent_id, occurred_at);
CREATE INDEX IF NOT EXISTS agent_usage_activity_occurred ON agent_usage_activity(occurred_at);
CREATE INDEX IF NOT EXISTS agent_usage_date ON agent_usage_records(agent_id, occurred_at);
CREATE INDEX IF NOT EXISTS agent_usage_occurred ON agent_usage_records(occurred_at);
CREATE INDEX IF NOT EXISTS agent_usage_skill_loads_date ON agent_usage_skill_loads(agent_id, occurred_at);
CREATE INDEX IF NOT EXISTS agent_usage_skill_loads_occurred ON agent_usage_skill_loads(occurred_at);
CREATE INDEX IF NOT EXISTS agent_waits_agent ON agent_waits(agent_id);
CREATE INDEX IF NOT EXISTS agent_waits_routine ON agent_waits(routine_id) WHERE routine_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS attachments_owner ON projection_attachments(owner_kind, owner_id);
CREATE INDEX IF NOT EXISTS browser_profile_proxies_proxy ON browser_profile_proxies(proxy_id);
CREATE UNIQUE INDEX IF NOT EXISTS browser_proxies_computer ON browser_proxies(member_id) WHERE kind = 'computer';
CREATE INDEX IF NOT EXISTS channel_assignments_channel ON projection_channel_assignments(channel_id);
CREATE INDEX IF NOT EXISTS channel_memories_channel ON projection_channel_memories(channel_id, updated_at DESC, memory_id);
CREATE INDEX IF NOT EXISTS channel_messages_sequence ON projection_channel_messages(channel_id, sequence);
CREATE UNIQUE INDEX IF NOT EXISTS channel_routine_runs_request ON projection_channel_routine_runs(request_message_id) WHERE request_message_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS channel_routine_runs_routine ON projection_channel_routine_runs(routine_id, created_at DESC, run_id);
CREATE INDEX IF NOT EXISTS channel_routine_triggers_due ON projection_channel_routine_triggers(next_run_at, routine_id);
CREATE INDEX IF NOT EXISTS channel_routines_channel ON projection_channel_routines(channel_id, updated_at DESC, routine_id);
CREATE INDEX IF NOT EXISTS channel_tasks_channel ON projection_channel_tasks(channel_id);
CREATE INDEX IF NOT EXISTS crm_activity_customer ON crm_activity(customer_id, created_at DESC, activity_id DESC);
CREATE INDEX IF NOT EXISTS crm_customer_tags_tag ON crm_customer_tags(tag_id);
CREATE UNIQUE INDEX IF NOT EXISTS crm_customers_source ON crm_customers(source_ref) WHERE source_ref IS NOT NULL;
CREATE INDEX IF NOT EXISTS crm_customers_status ON crm_customers(status_id);
CREATE INDEX IF NOT EXISTS crm_customers_stripe ON crm_customers(stripe_customer_id) WHERE stripe_customer_id <> '';
CREATE INDEX IF NOT EXISTS crm_payments_customer ON crm_payments(customer_id, due_on);
CREATE INDEX IF NOT EXISTS crm_payments_open ON crm_payments(due_on, number) WHERE paid_at IS NULL;
CREATE INDEX IF NOT EXISTS crm_payments_paid ON crm_payments(paid_at, number) WHERE paid_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS crm_payments_plan ON crm_payments(plan_id) WHERE plan_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS crm_payments_source ON crm_payments(source_ref) WHERE source_ref IS NOT NULL;
CREATE INDEX IF NOT EXISTS crm_plans_customer ON crm_plans(customer_id);
CREATE UNIQUE INDEX IF NOT EXISTS crm_plans_source ON crm_plans(source_ref) WHERE source_ref IS NOT NULL;
CREATE INDEX IF NOT EXISTS crm_proposals_agent ON crm_proposals(agent_id) WHERE state = 'pending';
CREATE INDEX IF NOT EXISTS crm_proposals_customer ON crm_proposals(customer_id) WHERE customer_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS crm_proposals_pending ON crm_proposals(created_at DESC, proposal_id DESC) WHERE state = 'pending';
CREATE INDEX IF NOT EXISTS direct_messages_thread ON projection_direct_messages(thread_id, last_event_sequence);
CREATE INDEX IF NOT EXISTS direct_threads_member_a ON projection_direct_threads(member_a_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS direct_threads_member_b ON projection_direct_threads(member_b_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_projection_messaging_history_place ON projection_messaging_history(connection_id, platform_channel_id, seq);
CREATE INDEX IF NOT EXISTS idx_projection_messaging_history_saved ON projection_messaging_history(saved_at);
CREATE INDEX IF NOT EXISTS idx_projection_messaging_history_sent ON projection_messaging_history(connection_id, platform_channel_id, sent_at);
CREATE UNIQUE INDEX IF NOT EXISTS mcp_servers_name ON projection_mcp_servers(name);
CREATE INDEX IF NOT EXISTS mcp_servers_order ON projection_mcp_servers(position, mcp_server_id);
CREATE INDEX IF NOT EXISTS messaging_memories_connection ON projection_messaging_memories(connection_id, updated_at DESC, memory_id);
CREATE INDEX IF NOT EXISTS messaging_threads_agent ON projection_messaging_threads(agent_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS orchestration_events_aggregate ON orchestration_events(aggregate_type, aggregate_id, sequence);
CREATE INDEX IF NOT EXISTS orchestration_events_command ON orchestration_events(command_id);
CREATE INDEX IF NOT EXISTS orchestration_events_thread_type ON orchestration_events(aggregate_id, event_type, sequence) WHERE aggregate_type = 'thread';
CREATE INDEX IF NOT EXISTS provider_history_imports_thread ON provider_history_imports(thread_id, provider, state);
CREATE INDEX IF NOT EXISTS provider_history_staging_pending ON provider_history_staging(session_id, imported, turn_id, item_index);
CREATE INDEX IF NOT EXISTS provider_history_staging_turn ON provider_history_staging(session_id, turn_id, item_index, item_key);
CREATE INDEX IF NOT EXISTS provider_history_turns_pending ON provider_history_turns(session_id, imported, turn_id);
CREATE INDEX IF NOT EXISTS provider_sessions_thread ON projection_provider_sessions(thread_id, provider, state);
CREATE INDEX IF NOT EXISTS routine_flow_links_to ON routine_flow_links(to_agent_id);
CREATE INDEX IF NOT EXISTS routine_flow_steps_agent ON routine_flow_steps(agent_id);
CREATE UNIQUE INDEX IF NOT EXISTS routine_flow_steps_delivery ON routine_flow_steps(delivery_id) WHERE delivery_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS routine_flow_steps_running ON routine_flow_steps(created_at, step_id) WHERE status = 'running';
CREATE INDEX IF NOT EXISTS routine_flow_steps_updated ON routine_flow_steps(updated_at);
CREATE INDEX IF NOT EXISTS routine_result_deliveries_routine ON routine_result_deliveries(routine_id, created_at DESC);
CREATE INDEX IF NOT EXISTS routine_result_deliveries_status ON routine_result_deliveries(status, next_attempt_at);
CREATE UNIQUE INDEX IF NOT EXISTS routine_runs_delivery ON projection_routine_runs(delivery_id) WHERE delivery_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS routine_runs_routine ON projection_routine_runs(routine_id, created_at DESC, run_id);
CREATE INDEX IF NOT EXISTS routine_runs_status_updated ON projection_routine_runs(status, updated_at);
CREATE INDEX IF NOT EXISTS routine_triggers_due ON projection_routine_triggers(next_run_at, routine_id);
CREATE INDEX IF NOT EXISTS thread_messages_order ON projection_thread_messages(thread_id, created_at, ordinal);
CREATE INDEX IF NOT EXISTS thread_messages_ordinal ON projection_thread_messages(thread_id, ordinal);
CREATE INDEX IF NOT EXISTS thread_messages_page_order ON projection_thread_messages(thread_id, created_at, ordinal, message_id);
CREATE INDEX IF NOT EXISTS thread_messages_shown_order ON projection_thread_messages(thread_id, group_start, group_first, group_id, turn_rank, created_at, ordinal, message_id, author, item_type);
CREATE INDEX IF NOT EXISTS thread_messages_turn ON projection_thread_messages(thread_id, turn_id);
CREATE INDEX IF NOT EXISTS webhook_receipts_received ON projection_webhook_receipts(received_at);
CREATE INDEX IF NOT EXISTS webhook_receipts_recent ON projection_webhook_receipts(owner_kind, routine_id, received_at DESC, receipt_id);
CREATE INDEX IF NOT EXISTS workflow_edges_from ON workflow_edges(workflow_id, from_node_id);
CREATE INDEX IF NOT EXISTS workflow_edges_to ON workflow_edges(workflow_id, to_node_id);
CREATE INDEX IF NOT EXISTS workflow_github_actions_item ON workflow_github_actions(repository, number, created_at);
CREATE INDEX IF NOT EXISTS workflow_github_actions_run ON workflow_github_actions(run_id);
CREATE INDEX IF NOT EXISTS workflow_github_deliveries_recent ON workflow_github_deliveries(workflow_id, node_id, received_at DESC);
CREATE INDEX IF NOT EXISTS workflow_github_deliveries_waiting ON workflow_github_deliveries(workflow_id, repository, number, received_at) WHERE outcome = 'waiting';
CREATE INDEX IF NOT EXISTS workflow_github_runs_item ON workflow_github_runs(repository, number, started_at);
CREATE INDEX IF NOT EXISTS workflow_github_runs_workflow ON workflow_github_runs(workflow_id);
CREATE INDEX IF NOT EXISTS workflow_nodes_agent ON workflow_nodes(agent_id) WHERE agent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS workflow_runs_open ON workflow_runs(status) WHERE status IN ('running', 'approval', 'stopping');
CREATE INDEX IF NOT EXISTS workflow_runs_recent ON workflow_runs(workflow_id, started_at DESC, run_id);
CREATE INDEX IF NOT EXISTS workflow_steps_agent ON workflow_steps(agent_id) WHERE agent_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS workflow_steps_attempt ON workflow_steps(run_id, node_id, attempt) WHERE run_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS workflow_steps_delivery ON workflow_steps(delivery_id) WHERE delivery_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS workflow_steps_open ON workflow_steps(status) WHERE status IN ('running', 'approval');
CREATE INDEX IF NOT EXISTS workflow_steps_solo ON workflow_steps(workflow_id, created_at DESC, step_id) WHERE run_id IS NULL;
CREATE INDEX IF NOT EXISTS workflow_triggers_due ON workflow_triggers(next_due_at) WHERE next_due_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS workflow_versions_recent ON workflow_versions(workflow_id, created_at DESC, version_id);
CREATE INDEX IF NOT EXISTS workflow_webhook_receipts_received ON workflow_webhook_receipts(received_at);

CREATE TRIGGER IF NOT EXISTS agent_memory_insert AFTER INSERT ON projection_agent_memories BEGIN
  INSERT INTO agent_memory_selections(memory_id) VALUES (new.memory_id);
  INSERT INTO agent_memory_search(rowid, text) VALUES (new.rowid, new.text);
END;

CREATE TRIGGER IF NOT EXISTS agent_memory_update AFTER UPDATE ON projection_agent_memories BEGIN
  INSERT INTO agent_memory_search(agent_memory_search, rowid, text) VALUES ('delete', old.rowid, old.text);
  INSERT INTO agent_memory_search(rowid, text) VALUES (new.rowid, new.text);
  UPDATE agent_memory_selections SET revision = revision + 1 WHERE memory_id = new.memory_id;
END;

CREATE TRIGGER IF NOT EXISTS agent_memory_delete AFTER DELETE ON projection_agent_memories BEGIN
  INSERT INTO agent_memory_search(agent_memory_search, rowid, text) VALUES ('delete', old.rowid, old.text);
END;

INSERT OR IGNORE INTO crm_statuses (status_id, label, tone, position, created_at) VALUES
  ('lead', NULL, 'info-light', 0, '1970-01-01T00:00:00.000Z'),
  ('customer', NULL, 'secondary', 1, '1970-01-01T00:00:00.000Z'),
  ('former', NULL, 'outline', 2, '1970-01-01T00:00:00.000Z');
"#;

pub const SCHEMA_V69_DDL_SECTIONS: [&str; 4] = [DDL_PART_1, DDL_PART_2, DDL_PART_3, DDL_PART_4];

/// Returns the concatenated `v69` SQLite DDL string across all 4 ordered sections.
pub fn schema_v69_ddl() -> String {
    SCHEMA_V69_DDL_SECTIONS.concat()
}

/// Executes all 4 `v69` DDL sections + built-in `crm_statuses` seed rows on `conn`.
pub fn execute_schema_v69_ddl(conn: &Connection) -> StorageResult<()> {
    for section in SCHEMA_V69_DDL_SECTIONS {
        conn.execute_batch(section)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct OpenBotMigrationOptions {
    pub applied_at: Option<String>,
    pub closed_cleanly: bool,
    pub through_version: Option<i64>,
}

impl Default for OpenBotMigrationOptions {
    fn default() -> Self {
        Self {
            applied_at: None,
            closed_cleanly: true,
            through_version: None,
        }
    }
}

pub fn has_existing_schema(conn: &Connection) -> StorageResult<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

pub fn read_applied_versions(conn: &Connection) -> StorageResult<Vec<i64>> {
    let has_table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if has_table.is_none() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    let mut rows = stmt.query([])?;
    let mut versions = Vec::new();
    while let Some(row) = rows.next()? {
        match row.get_ref(0)? {
            ValueRef::Integer(v) => versions.push(v),
            _ => {
                return Err(StorageError::validation(
                    "OpenBot database contains an invalid schema migration version.",
                ));
            }
        }
    }
    Ok(versions)
}

pub fn validate_applied_versions(versions: &[i64]) -> StorageResult<()> {
    let newest = versions.last().copied().unwrap_or(0);
    if newest > LATEST_SCHEMA_VERSION {
        return Err(StorageError::NewerDatabase {
            found: newest,
            supported: LATEST_SCHEMA_VERSION,
        });
    }
    let baseline_and_later: HashSet<i64> = versions
        .iter()
        .copied()
        .filter(|&version| version >= BASELINE_SCHEMA_VERSION)
        .collect();
    if baseline_and_later.is_empty() {
        return Ok(());
    }
    for version in BASELINE_SCHEMA_VERSION..=newest {
        if !baseline_and_later.contains(&version) {
            return Err(StorageError::MigrationHistoryGap {
                missing_version: version,
            });
        }
    }
    Ok(())
}

pub fn latest_applied_version(versions: &[i64]) -> i64 {
    versions
        .iter()
        .copied()
        .filter(|&version| version >= BASELINE_SCHEMA_VERSION)
        .max()
        .unwrap_or(BASELINE_SCHEMA_VERSION - 1)
}

pub fn openbot_database_needs_migration(conn: &Connection) -> StorageResult<bool> {
    if !has_existing_schema(conn)? {
        return Ok(true);
    }
    let applied = read_applied_versions(conn)?;
    Ok(latest_applied_version(&applied) < LATEST_SCHEMA_VERSION)
}

pub fn read_foreign_key_violations(conn: &Connection) -> StorageResult<BTreeMap<String, usize>> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    let mut counts = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let table: String = row.get(0).unwrap_or_else(|_| "unknown".to_string());
        let parent: String = row.get(2).unwrap_or_else(|_| "unknown".to_string());
        let reference = format!("{table} -> {parent}");
        *counts.entry(reference).or_insert(0) += 1;
    }
    Ok(counts)
}

pub fn assert_foreign_keys(
    conn: &Connection,
    before: &BTreeMap<String, usize>,
    migration: i64,
) -> StorageResult<BTreeMap<String, usize>> {
    let violations = read_foreign_key_violations(conn)?;
    let mut added: Vec<(&str, usize)> = Vec::new();
    for (reference, &count) in &violations {
        let prior = before.get(reference).copied().unwrap_or(0);
        if count > prior {
            added.push((reference.as_str(), count - prior));
        }
    }
    if !added.is_empty() {
        let total: usize = added.iter().map(|(_, delta)| *delta).sum();
        let references = added
            .iter()
            .map(|(reference, _)| *reference)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(StorageError::ForeignKeyViolation {
            migration,
            details: format!(
                "OpenBot database migration produced {total} foreign-key violation(s) in {references}."
            ),
        });
    }
    Ok(violations)
}

pub fn assert_quick_check(conn: &Connection) -> StorageResult<()> {
    let mut stmt = conn.prepare("PRAGMA quick_check(20)")?;
    let mut rows = stmt.query([])?;
    let mut problems = Vec::new();
    while let Some(row) = rows.next()? {
        problems.push(row.get::<_, String>(0)?);
    }
    if problems.len() == 1 && problems[0] == "ok" {
        return Ok(());
    }
    Err(StorageError::IntegrityCheckFailed(problems.join("; ")))
}

/// Runs `openbot.db` schema initialization or migration up to `options.through_version` (`v69` by default).
pub fn migrate_openbot_database(
    conn: &Connection,
    options: &OpenBotMigrationOptions,
) -> StorageResult<()> {
    let applied_at = options
        .applied_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
    let is_new_database = !has_existing_schema(conn)?;
    let applied_versions = read_applied_versions(conn)?;
    validate_applied_versions(&applied_versions)?;

    let current_version = latest_applied_version(&applied_versions);
    let through_version = options
        .through_version
        .unwrap_or(LATEST_SCHEMA_VERSION)
        .min(LATEST_SCHEMA_VERSION);
    let has_pending = current_version < through_version;

    if has_pending {
        let violations_before = read_foreign_key_violations(conn)?;
        let nested = !conn.is_autocommit();
        if nested {
            conn.execute_batch("SAVEPOINT openbot_schema_migration_sp;")?;
        } else {
            conn.execute_batch("BEGIN IMMEDIATE;")?;
        }

        let step_result = (|| -> StorageResult<()> {
            if !is_new_database {
                upgrade_existing_tables_to_v69(conn, current_version)?;
            }
            execute_schema_v69_ddl(conn)?;
            let start_version = current_version.max(BASELINE_SCHEMA_VERSION - 1) + 1;
            let mut insert_ver = conn.prepare(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            )?;
            for version in start_version..=through_version {
                insert_ver.execute(params![version, applied_at])?;
            }
            drop(insert_ver);
            assert_foreign_keys(conn, &violations_before, through_version)?;
            Ok(())
        })();

        match step_result {
            Ok(()) => {
                if nested {
                    conn.execute_batch("RELEASE SAVEPOINT openbot_schema_migration_sp;")?;
                } else {
                    conn.execute_batch("COMMIT;")?;
                }
            }
            Err(err) => {
                if !conn.is_autocommit() {
                    let _ = if nested {
                        conn.execute_batch(
                            "ROLLBACK TO SAVEPOINT openbot_schema_migration_sp; \
                             RELEASE SAVEPOINT openbot_schema_migration_sp;",
                        )
                    } else {
                        conn.execute_batch("ROLLBACK;")
                    };
                }
                return Err(err);
            }
        }
    }

    if has_pending || !options.closed_cleanly {
        assert_quick_check(conn)?;
    }

    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> StorageResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name.eq_ignore_ascii_case(column) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> StorageResult<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
            params![table],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, alter_tail: &str) -> StorageResult<()> {
    if table_exists(conn, table)? && !table_has_column(conn, table, column)? {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {alter_tail};"))?;
    }
    Ok(())
}

fn upgrade_existing_tables_to_v69(conn: &Connection, current_version: i64) -> StorageResult<()> {
    if current_version < 47 && table_exists(conn, "projection_agent_tasks")? {
        conn.execute_batch("DROP TABLE IF EXISTS projection_agent_tasks;")?;
    }
    ensure_column(
        conn,
        "projection_agent_routines",
        "limit_policy",
        "TEXT NOT NULL DEFAULT 'wait' CHECK(limit_policy IN ('wait', 'skip'))",
    )?;
    ensure_column(
        conn,
        "projection_agent_routines",
        "missed_policy",
        "TEXT NOT NULL DEFAULT 'skip' CHECK(missed_policy IN ('skip', 'run-once'))",
    )?;
    ensure_column(
        conn,
        "projection_agent_routines",
        "run_in",
        "TEXT NOT NULL DEFAULT 'chat' CHECK(run_in IN ('chat', 'thread'))",
    )?;
    ensure_column(
        conn,
        "projection_channel_routines",
        "limit_policy",
        "TEXT NOT NULL DEFAULT 'wait' CHECK(limit_policy IN ('wait', 'skip'))",
    )?;
    ensure_column(
        conn,
        "projection_channel_routines",
        "missed_policy",
        "TEXT NOT NULL DEFAULT 'skip' CHECK(missed_policy IN ('skip', 'run-once'))",
    )?;
    ensure_column(
        conn,
        "projection_routine_runs",
        "missed_count",
        "INTEGER CHECK(missed_count IS NULL OR missed_count >= 1)",
    )?;
    ensure_column(conn, "projection_routine_runs", "missed_until", "TEXT")?;
    ensure_column(
        conn,
        "projection_routine_runs",
        "run_source",
        "TEXT CHECK(run_source IS NULL OR run_source IN ('test', 'script', 'webhook'))",
    )?;
    ensure_column(
        conn,
        "projection_channel_routine_runs",
        "missed_count",
        "INTEGER CHECK(missed_count IS NULL OR missed_count >= 1)",
    )?;
    ensure_column(conn, "projection_channel_routine_runs", "missed_until", "TEXT")?;
    ensure_column(
        conn,
        "projection_channel_routine_runs",
        "run_source",
        "TEXT CHECK(run_source IS NULL OR run_source IN ('test', 'script', 'webhook'))",
    )?;
    ensure_column(conn, "projection_thread_messages", "group_start", "TEXT")?;
    ensure_column(conn, "projection_thread_messages", "group_first", "INTEGER")?;
    ensure_column(conn, "projection_thread_messages", "group_id", "TEXT")?;
    ensure_column(conn, "projection_thread_messages", "turn_rank", "INTEGER")?;
    ensure_column(conn, "projection_agent_threads", "branch_source_thread_id", "TEXT")?;
    ensure_column(conn, "projection_agent_threads", "branch_source_message_id", "TEXT")?;
    ensure_column(conn, "projection_agent_threads", "branch_through_message_id", "TEXT")?;
    ensure_column(conn, "projection_agent_threads", "routine_id", "TEXT")?;
    ensure_column(
        conn,
        "projection_agent_memories",
        "site",
        "TEXT CHECK(site IS NULL OR length(site) BETWEEN 1 AND 253)",
    )?;
    ensure_column(
        conn,
        "workflow_steps",
        "settings_json",
        "TEXT CHECK(settings_json IS NULL OR json_valid(settings_json))",
    )?;
    ensure_column(
        conn,
        "workflow_steps",
        "sources_json",
        "TEXT CHECK(sources_json IS NULL OR json_valid(sources_json))",
    )?;
    ensure_column(
        conn,
        "workflows",
        "paused",
        "INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0, 1))",
    )?;
    ensure_column(
        conn,
        "projection_messaging_allowed_senders",
        "mode",
        "TEXT NOT NULL DEFAULT 'reply' CHECK(mode IN ('reply', 'read'))",
    )?;
    Ok(())
}
