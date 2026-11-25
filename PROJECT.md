# Project: Orca Pure-Rust Absorption into `gpui-telegram` (Fabushi v1.7.0)

- **Repository**: `/home/antigravity/bhrumom/gpui-telegram` (`origin`: `https://github.com/bhrumom/gpui-telegram.git`, baseline `cc56925` / `v1.6.11`)
- **Upstream Reference**: `https://github.com/stablyai/orca` (audited commit `e0292cf4e836e29c261d94246be4bb19abfca03a`)
- **Authoritative Request**: `/home/antigravity/bhrumom/gpui-telegram/.agents/teamwork/ORIGINAL_REQUEST.md` (`## Follow-up — 2026-11-25T10:09:22Z`)
- **Survey Artifacts**:
  - `.agents/teamwork/orca_survey_spec_miner_1/orca_core_spec.md`
  - `.agents/teamwork/orca_survey_spec_miner_2/orca_bots_and_designs_spec.md`
  - `.agents/teamwork/orca_survey_explorer_3/gpui_telegram_survey.md`

---

## Architecture

Absorb `stablyai/orca`'s complete **Bot Operating System**, **Multi-Agent Orchestration Runtime**, and **Agent IDE Architecture** natively in pure Rust + GPUI `0.2.2` inside `gpui-telegram` (Fabushi), with **zero** delegation to Electron/Node/JS/TS Orca runtimes and **zero** regressions to the frozen 3,441-file `tdesktop` parity ledger (`scripts/verify_ledger_closure.py`).

```text
Layer 0 — Pure Domain & Contracts (crates/orca_core)
  BotSchema (22 agents), BotHome/Preamble/NO_REPLY, Memory Provenance/Protected/TidyUp/ReAddGuard,
  BotItems (6 states + Ask cards + NeedsYou fold), LookAround, BotGroups + RequestPolicy,
  24 Bot Templates + 13 Skills + BotPackage v1 + 5-Class Scanner + E2EE Team Share,
  31 Event Triggers + 6-Adapter Webhooks + OneShot Waits + WatchPR,
  Strict orca.yaml + .worktreeinclude + FolderWorkspaces + WorkspaceLayoutModel (10 tab types),
  Orchestration DAG/Mailbox/Gates/Federation, Unified RPC + Protocol v3 + E2EE v2 HKDF-SHA256,
  DiffAnnotation/AI Attribution, DesignMode, PluginV2 Manifest, Hibernation & ComputerUse Guards
         │
         ├───────────────────────────────┐
         ▼                               ▼
Layer 1A — Bot OS Engine           Layer 1B — Agent IDE & Orchestration Engine
(crates/orca_bots)                 (crates/orca_ide + narrow gpui_telegram_coding adapters)
  BotController (serialized),        SQLite OrchestrationEngine (Runs/Tasks/Dispatches/Mailbox/
  BotHomeStore (CAS + bot.json),     Gates/Federation/Push-Wake), 22+ CLI Agent Catalog +
  BotMemoryService (Provenance,      Event-Sourced Chat Journal + Chat<->Terminal Continuation,
  Journal, 3AM-8AM TidyUp, 14d       4-State Agent Hook Server + ReDoS-safe JSON Rules,
  ReAddGuard), SQLite BotItemsStore, Parallel Worktrees + FolderWorkspaces + orca.yaml Runner +
  LookAroundEngine, SQLite           *.orca.localhost Proxy + First-Work Auto-Rename,
  BotGroupRuntime (rollback triggers), Detached PTY Cold-Park/ACK Flow Control + 8-Check
  Axum 127.0.0.1 BotToolRelay +      Hibernation + Multi-Account Hot-Swap + 9-Provider Usage,
  Host OAuth 2.1 + SharedMcpPool,    ProfileState Authority + Tantivy/FTS AI Vault Search,
  Connectors/ActsFor/AccessHost/     DiffAnnotationStore + AI Attribution, PluginHostV2,
  SlackMirror/WebhookEngine/Waits,   MCP/Skills Sync, Safe ComputerUse/Emulator, STT Dictation,
  Templates/Packages/E2EE Share/     Shareable Artifacts, Ephemeral VM Recipes, Hang Watchdog,
  Cross-App Import/HandOver/ChatTree Unified RPC Dispatcher
         │                               │
         └───────────────┬───────────────┘
                         ▼
Layer 2 — Network, Server, Client & CLI (gpui_telegram_proto/server/client/cli)
  REST /api/v1/orca/*, Webhook Ingress, Content-Free Wake WS, E2EE Relay, Telegram IM Fusion,
  Unified RPC Endpoint, Typed OrcaClient, Agent-Ergonomic `fabushi` CLI (`--json`)
                         │
                         ▼
Layer 3 — Native GPUI Desktop & Browser (gpui_telegram_desktop + bot-browser)
  Bots Sidebar + Bot Chat Tree + Group Rooms, Bot Home (Chat/Items/UpNext/Settings) +
  Priority-Folded "Needs You" Inbox + 10 Bot Cards + 24-Template Gallery + Hand-Over Dialog,
  Split Anything Panes + Cmd+J Scoped Jump Palette + Chat<->Terminal Flip (Cmd+Shift+J) +
  Orchestration/Kanban Board + Annotate AI Diff + Browser Design Mode + Usage Popover +
  SecuritySurface Non-Themable Modals + AI Vault + Plugin V2 UI
```

---

## Code Layout

- **Existing Workspace Convention**: 22 Rust workspace members under `crates/*`, `tests/*`, `inherited/*` (`Cargo.toml`, Rust edition `2024`, lean profiles `debug = 0`, `strip = "debuginfo"`, `incremental = false`).
- **Ledger Protection Invariant**: **NEVER** delete, rename, or remove symbols from existing `tdesktop` ledger-mapped `.rs` files (`scripts/verify_ledger_closure.py --ci-without-upstream --skip-cargo` MUST pass all 6 gates).
- **Mandatory Post-Build Hygiene (`AGENTS.md`)**: Run `python3 scripts/clean_build_garbage.py` immediately after every `cargo check`, `cargo build`, or `cargo test`.
- **New Workspace Crates & Modules**:
  - `crates/orca_core/` — Pure domain models, parsers, scanners, crypto, templates, contracts (leaf crate; zero SQLite/network I/O)
  - `crates/orca_bots/` — Stateful Orca Bot OS engine (SQLite WAL stores, FS homes, Axum loopback Tool Relay, OAuth 2.1, Shared MCP pool, Triggers/Webhooks, Sharing/Import/Handover)
  - `crates/orca_ide/` — Stateful Orca Orchestration, Extended Agent Catalog, Chat Journal, Hook Server, Worktrees/`orca.yaml`/`*.orca.localhost` Proxy, PTY/Hibernation/Usage, AI Vault, Diff Annotations, Plugin V2, ComputerUse/Emulator/Dictation/Artifacts/VM/Watchdog, Unified RPC Dispatcher
  - `crates/gpui_telegram_coding/src/driver/` — Narrow new CLI agent driver adapters (preserving existing 6 providers intact)
  - `crates/gpui_telegram_proto/src/orca_contracts.rs` — Wire contracts for REST/WS/RPC
  - `crates/gpui_telegram_server/src/routes/orca_*.rs` — Server HTTP/WS/Webhook/Wake/Share routes
  - `crates/gpui_telegram_client/src/net/orca_client.rs` — Typed async Orca client
  - `crates/gpui_telegram_cli/src/orca_commands.rs` — `fabushi` CLI Orca subcommands (`--json`)
  - `crates/bot-browser/src/design_mode.rs` — CDP/DOM Design Mode element grabber
  - `crates/gpui_telegram_desktop/src/views/orca/**` & `views/browser/design_mode.rs` — Native GPUI views
  - `tests/orca_e2e/` — Opaque-box E2E test infrastructure & Tiers 1–4 (+ Tier 5 adversarial) suite

---

## Feature Inventory

Every feature discovered across Spec Miner 1 (32 rows), Spec Miner 2 (23 groups / 26 edge cases), and Explorer 3 (workspace invariants & release pipeline) is assigned a Milestone (zero unassigned).

| # | Feature | Description | Milestone | Source |
|---|---|---|---|---|
| F01 | Bot Identity, 22-Agent Schema & CAS Avatars | `OrcaBotRecord`, 22 `BOT_AGENTS` + retired migration, `BotName` (`\p{Cc}`/<=64), slug regex, 20 mascots + 9 outfits, CAS `avatarRevision`/`Epoch`, rollback split, safe-empty `googleAccounts` | M1 | survey (SM2 §1.1, §3 #1) |
| F02 | Bot Home Generator, Authority Preamble, First-Run & `NO_REPLY` | `SOUL.md`/`MEMORY.md`/`AGENTS.md`/`CLAUDE.md`/`bot.json`, `Tools` ahead of `SOUL.md` & 200-line fenced `MEMORY.md`, `firstRun` rules, SHA-256 delta sync + overflow guard, streaming `NO_REPLY` suppression | M1, M2 | survey (SM2 §1.2–1.3, §1.10) |
| F03 | Host-Enforced Memory Governance, Journal, Tidy-Up & 14d Re-Add Guard | `· from:` provenance + separator defusing, silent-delete restore, `forget` + Undo, protected `Rule:`/`Prefers:`/`Decided:` & standing words, deterministic `memory/journal/YYYY-MM.md`, tool-less 3–8 AM Tidy-Up + Undo, 14-day Re-Add Guard (archive-before-remove) | M1, M2 | survey (SM2 §1.4, §3 #3–5) |
| F04 | Event-Sourced Bot Items, Multi-Decision Ask Cards & `"Needs You"` Inbox | SQLite `items`/`item_links`/`item_events`, 6 states, owner-cancel guard (`item_closed`), snooze + early unsnooze, stale (3x planned), `--ask/--option/--recommended/--multi` cards, lineage-deduped links, `mail='owed'`, priority-folded `"Needs you"`, `items_read_only` forward-compat | M1, M2 | survey (SM2 §1.5, §3 #6; E3 §3.1) |
| F05 | Proactive Read-Only `"Look Around"` -> `"Up Next"` | Daytime schedule presets (`every-4h/2h/twice-daily/mornings`), weekdays, sleep `dueNow`, read-only agent guard, <=5 deduped suggestions (GitHub/Linear URL norm), `linkCarriesCredential` stripper | M1, M2 | survey (SM2 §1.6, §3 #7) |
| F06 | Multi-Bot Group Rooms, Sub-Threads, Peer Mail & Remote Bot Members | SQLite `bot_groups`/`bot_group_messages` with rollback-safe `AFTER UPDATE` triggers, sub-threads, `bot|all|mentions` reply modes, `@mention` handoffs, `BotGroupPeerScope`, `remoteMembers`/`remoteAsks`, paused-bot mail hold, 7-code request policy | M1, M2 | survey (SM2 §1.7, §3 #8) |
| F07 | 24 Bot Templates, 13 Skills, Packages, E2EE Team Share, Import & Hand-Over | 6-category 24 templates + 13 `SKILL.md`, v1 `BotPackage` + 5-class text scanner (`secret` blocks), Curve25519+Ed25519 `share-send` E2EE team sharing, OpenClaw/Hermes/OpenBot/export zero-parser import + redaction, transactional `move->notify->restore` Hand-Over, Bot Chat Tree, 10 Bot Cards, Usage-Limit Auto-Continue | M1, M2 | survey (SM2 §1.10, §3 #12–13) |
| F08 | Loopback Credential-Isolating Tool Relay, Host OAuth 2.1 & Shared MCP Pool | `127.0.0.1` Axum relay (Origin/Host 403 CSRF, Bearer 401/404, 429 admission, upstream 401 single refresh retry, immediate revoke abort, secret scrub), OAuth 2.1 PKCE/DCR/RFC8707/9207/7009 + SSRF guard, SHA-256 `SharedMcpProcesses` stdio pool (10s failed-start debounce) | M2 | survey (SM2 §1.8, §3 #9) |
| F09 | External Connectors, `"Acts For"`, Access Control, Slack Mirror, Triggers, Webhooks & Waits | `actsFor` attribution (`"asked by <name> via <Bot>"`), Access Request approval/redeem (`Who Can Ask`), Slack Mirror turn-end ordering barrier, `--leave-thread` anti-injection, 31 event triggers (6 sources), 6-adapter JSON-path webhooks (200/300 hr caps, 14 states, zero-leak wire), `--once [--by]` waits, `watch-pr` 2-attempt loop, content-free Wake WS | M1, M2, M4 | survey (SM2 §1.9, §3 #10–11) |
| F10 | Strict `orca.yaml`, `.worktreeinclude`, Folder Workspaces & Parallel Worktrees | First-class Folder Workspaces, Git worktree lineage + interrupted-removal recovery + sparse presets, strict `orca.yaml` (reject duplicate keys, >256KiB, >100 aliases, `setupTrustHash`), `.worktreeinclude` (APFS/copy, 2GiB/50k cap), Checkpoints, First-Work Auto-Rename, `*.orca.localhost` Label Proxy, Disk Space Cleanup | M1, M3 | survey (SM1 §3 #1–6; SM2 §2.6) |
| F11 | Durable SQLite Multi-Agent Orchestration & Cross-Host Federation | `Runs`, `Tasks` DAG, `DispatchContexts`, `WorkerDispatches`, `WorkerTerminalResources`, 9-type `Mailbox` + group addressing (`@all/@idle/@agent/@worktree`), `Deliveries`, `Questions`, `DecisionGates`, idempotent `MutationReceipts`, cross-host `--on <server>` federation, push-wake (zero polling), non-guessing `Unverifiable` liveness, auto-teardown & reopen | M1, M3 | survey (SM1 §2.6, §3 #19; SM2 §2.1) |
| F12 | Extended 22+ CLI Agents, Event-Sourced Chat Journal & Chat<->Terminal Continuation | 22+ TUI/ACP agent catalog + dynamic model/effort probing, event-sourced SQLite `AgentSessionJournal` with idempotent command receipts + queued submissions + `providerHandleChain`, bidirectional Chat<->Terminal switch (`Cmd+Shift+J`) & history fork | M3 | survey (SM1 §2.3, §3 #14–15; SM2 §2.2) |
| F13 | 4-State Agent Status Engine, Hook Server, Screen-Anchor Rules & Safe Hibernation | Loopback `AgentHookServer` + restart-surviving `endpoint.env`, linear-time JSON screen-anchor rules, OSC 7501 facets, `sessionBoundary` suppression, live subagents, prompt-cache TTL; 8-condition background PTY hibernation + transparent `--resume` | M1, M3 | survey (SM1 §2.6, §3 #16; SM2 §2.7) |
| F14 | Detached PTY Persistence, Cold-Park, Credit/ACK Multiplex Flow Control & Tombstones | Headless VT snapshot serialization, daemon `DaemonTransientFact` side-effect extraction, hidden view parking, round-robin credit/ACK multiplex flow control + `ack-pending-overflow` recovery snapshots, `PtyIncarnationId` + 30-day closed-tab tombstones | M1, M3 | survey (SM1 §2.4, §3 #11–13) |
| F15 | Multi-Account Hot-Swap & 9-Provider Usage / Rate-Limit Roster | Isolated per-account runtime homes + live hot-swap (integrating `magpie_gateway`), 9-provider quota/reset tracker sorted tightest-first with 80% warning chip & usage-limit stop/auto-continue | M3, M5 | survey (SM1 §3 #18; SM2 §2.7) |
| F16 | Domain-Partitioned ProfileState SQLite, AtomicSnapshotWriter, Keyring & AI Vault FTS | Optimistic-revision domain-partitioned SQLite ProfileState, generation-fenced `AtomicSnapshotWriter`, `openVersionedCacheDb` auto-rebuild on `SQLITE_CORRUPT`, non-destructive `ProtectedSecretPersistence`; incremental `byte_offset` + SHA-256 `prefix_hash` AI Vault search (camelCase/snake_case + typo repair + fork/handoff) | M1, M3 | survey (SM1 §2.3, §3 #17; SM2 §2.9) |
| F17 | Annotate AI Diff, Line-Level AI Attribution, Source Control & Tasks Hub | Line-anchored persistent diff review comments tracking line shifts + single-batch `"Send to agent"`, local per-line AI-vs-Human gutter attribution (never pollutes commits), AI commit/PR drafting, multi-forge Tasks Hub (GitHub/GitLab/Linear/Jira) | M1, M3, M5 | survey (SM1 §3 #21–23; SM2 §2.3) |
| F18 | Embedded Browser Design Mode, Safe Computer Use & Mobile Emulator | `bot-browser` Design Mode element picker (DOM + computed CSS + cropped PNG + source map -> active chat); background-delivery AX Computer Use (`treeDiff`, `system_shortcut_blocked` guard); Mobile Emulator normalized-coord/AX bridge | M1, M3, M5 | survey (SM1 §3 #24–25; SM2 §2.4, §2.9) |
| F19 | 4-Tier Capability-Gated Plugin System V2, MCP Sync & Transactional Skills | `data`/`declarative`/`sandboxed-ui`/`trusted-worker` tiers, 16 contribution points, capability-fingerprint approval, 2s action-replacement fallback, `onError: pass|deny` hooks, `SecuritySurface` anti-spoofing, MCP server config sync, transactional skill installer | M1, M3, M5 | survey (SM1 §3 #28–29; SM2 §2.9) |
| F20 | Unified RPC Dispatcher, Capability Protocol v3, E2EE v2 Relay/Wake, Ephemeral VMs, STT, Artifacts & Watchdog | Single typed RPC dispatcher with caller scopes & capability negotiation, transcript-bound X25519 + HKDF-SHA256 96B E2EE v2, headless `serve` mode + Ephemeral VM recipe runner/doctor, local/cloud STT dictation + GPU fallback marker, Shareable Artifacts with hidden edit tokens, sleep-aware Hang Watchdog | M1, M3, M4 | survey (SM1 §2.2, §3 #26–27, #30; SM2 §2.8–2.9) |
| F21 | Server REST/WS/Webhook/Wake API, Telegram IM Bridge Fusion, Typed Client & `fabushi` CLI | `/api/v1/orca/*` routes, Telegram Bot/Chat fusion for Orca Bots & Decision Cards, `OrcaClient`, and full agent-ergonomic `--json` `fabushi` CLI (`bot`, `orchestration`, `webhooks`, `automations`, `worktree`, `browser`, `computer`, `artifacts`, `plugin`, `serve`, `profile`) | M4 | survey (SM1 §3 #31–32; E3 §3.3) |
| F22 | Native GPUI Desktop UI (Bots Surface, Needs You, Split Anything, Cmd+J, Diff, Design Mode, Kanban) | Bots Sidebar + Bot Chat Tree + Group Rooms, 4-tab Bot Home + 10 Bot Cards + 24-Template Gallery + Hand-Over Dialog, Split Anything recursive panes (10 tab types + pinned boundaries), `Cmd+J` Scoped Jump Palette (`Cmd+1..6` + `Tab` chips), `Cmd+Shift+J` Chat<->Terminal flip, Orchestration/Kanban Board + Pop-out, Annotate AI Diff, Design Mode overlay, Usage Popover, `SecuritySurface` | M5 | survey (SM1 §2.5, §3 #7–10; SM2 §2.5, §4.2; E3 §3.3) |
| F23 | Opaque-Box E2E Suite (Tiers 1–4) + Tier 5 Adversarial Hardening | Requirement-driven opaque-box E2E test infrastructure (`TEST_INFRA.md` -> `TEST_READY.md`) covering all features across Tiers 1–4, followed by white-box Tier 5 Challenger-driven adversarial hardening | E2E Track, M6 | Project Pattern Dual Track |
| F24 | Ledger Preservation, CI/Deploy Workflows, Version `v1.7.0` Bump, Git Push & GitHub Release | Maintain 6-gate `verify_ledger_closure.py`, update `ci.yml` & `deploy-backend.yml`, bump `Cargo.toml:44` + `FabushiInstaller.swift:13` + `FabushiSetup.iss:5` to `1.7.0`, push `main`, tag `v1.7.0`, verify CI + Release `Fabushi v1.7.0` | M4, M7 | survey (E3 §1.4, §4.4; ORIGINAL_REQUEST) |

---

## Milestones

| # | Name | Scope | Dependencies | Status |
|---|---|---|---|---|
| M0 (Prereq) | Devbox Linux Toolchain Readiness | Install `mold`, `protobuf-compiler`, and GPUI/PipeWire `-dev` APT libs (`.github/workflows/ci.yml:63`), verify baseline `cargo check -p gpui_telegram_proto` + `clean_build_garbage.py` | none | DONE (`b1e5d797972fa9acfac752b52bf90b12`; mold/protoc/GPUI libs + baseline check verified) |
| M1 | `orca_core` Pure Domain, Contracts, Templates, Parsers & Security | `crates/orca_core/**`, root `Cargo.toml` (add `orca_core`). Covers F01–F10 pure contracts/parsers/crypto/scanners/templates/layout/orchestration/RPC/E2EE v2/plugin/diff/hibernation/computer-use types + unit tests | M0 | DONE (`649ff8148177741627dd79cb6f64cf47`; 19 files, 138/138 tests PASS `--offline`, 0 clippy warnings, 0 stubs) |
| M2 | `orca_bots` Full Stateful Orca Bot Operating System | `crates/orca_bots/**`, root `Cargo.toml` (add `orca_bots`). Covers F02–F09 stateful engines: BotController, BotHomeStore, BotMemoryService (provenance, journal, 3–8 AM Tidy-Up, 14d Re-Add Guard), SQLite BotItemsStore + NeedsYou + Ask cards, LookAround, SQLite BotGroupRuntime (rollback triggers + remote bots + 7-code policy), Axum `127.0.0.1` BotToolRelay + OAuth 2.1 + SharedMcpPool, Connectors/ActsFor/AccessHost/SlackMirror/Webhooks/31 Triggers/Waits/WatchPR, Templates/Packages/E2EE Share/Import/HandOver/ChatTree | M1 | IN_PROGRESS (`80677f528972f86716b3b63e3b102102`) |
| M3 | `orca_ide` Orchestration, Extended Agents, Worktrees, PTY/Vault/Plugins/Review | `crates/orca_ide/**`, root `Cargo.toml` (add `orca_ide`), narrow new adapters in `crates/gpui_telegram_coding/src/driver/`. Covers F10–F20 stateful engines: SQLite OrchestrationEngine + Federation, 22+ CLI Agents + Event-Sourced Chat Journal + Chat<->Terminal, 4-State Hook Server + ReDoS-safe Rules, Folder Workspaces + `orca.yaml`/`.worktreeinclude` + `*.orca.localhost` Proxy + First-Work Rename, PTY Cold-Park/ACK Flow Control + 8-Check Hibernation + Multi-Account/9-Provider Usage, ProfileState + AI Vault FTS + Diff Annotations/Attribution + PluginHostV2 + MCP/Skills Sync + ComputerUse/Emulator/STT/Artifacts/VM/Watchdog + Unified RPC Dispatcher | M2 | PLANNED |
| M4 | Server API, Client Bridge, Telegram IM Fusion, `fabushi` CLI & CI/Deploy | `gpui_telegram_proto/src/orca_contracts.rs`, `gpui_telegram_server/src/routes/orca_*.rs`, `gpui_telegram_client/src/net/orca_client.rs`, `gpui_telegram_cli/src/orca_commands.rs`, `.github/workflows/{ci,deploy-backend}.yml`. Covers F09, F20, F21, F24 (CI/deploy updates) | M2, M3 | PLANNED |
| M5 | Native GPUI Desktop UI Surfaces & `bot-browser` Design Mode | `crates/bot-browser/src/design_mode.rs`, `crates/gpui_telegram_desktop/src/views/orca/**`, `views/browser/design_mode.rs`, narrow wiring in `nav_rail.rs`/`shell.rs`/`shell_shortcuts.rs`. Covers F15, F17, F18, F19, F22 | M4 | PLANNED |
| M6 | Final Milestone: E2E Suite Pass (Tiers 1–4) & Tier 5 Adversarial Hardening | Phase 1: pass 100% of `TEST_READY.md` E2E suite sequentially (Tier 1 -> Tier 2 -> Tier 3 -> Tier 4, whole-project Forensic Audit on Tier 4). Phase 2: white-box Tier 5 Challenger-driven adversarial hardening loop until zero gaps remain. Covers F23 | M5, `TEST_READY.md` | PLANNED |
| M7 | Release `v1.7.0`, Ledger Audit, Git Push & GitHub Release Publication | Bump `Cargo.toml:44`, `scripts/macos/FabushiInstaller.swift:13`, `scripts/windows/FabushiSetup.iss:5` to `1.7.0` + `Cargo.lock`; full `verify_ledger_closure.py` (6/6 gates PASS) + full workspace tests + `clean_build_garbage.py`; push `main`; tag & push `v1.7.0`; verify `ci.yml`, `deploy-backend.yml`, `release.yml` & `Fabushi v1.7.0` release. Covers F24 | M6 | PLANNED |
| E2E Track | Independent Opaque-Box E2E Testing Track | `TEST_INFRA.md`, `tests/orca_e2e/**`, `TEST_READY.md`. Requirement-driven opaque-box test harness & Tiers 1–4 cases covering F01–F24 (>=120 T1, >=120 T2, >=24 T3, >=12 T4, total >=276 cases) against Interface Contracts below | M0 | DONE (`0fef006d381393e69d17494b5a23fc80`; 5/5 unanimous pass; `TEST_READY.md` published — 330 T1–T4 + 23 self = 353 tests, 24/24 features, 40/40 edge cases) |

---

## Interface Contracts

### 1. `orca_core` ↔ `orca_bots` & `orca_ide` (Pure Rust Types & Error Codes)
- **Bot Error Codes** (`orca_core::BotErrorCode` serialized snake_case): `bot_name_taken`, `bot_not_found`, `bot_folder_not_owned`, `bot_avatar_changed`, `bot_avatar_invalid`, `invalid_argument`, `bots_disabled`, `bot_chat_not_found`, `bot_paused`, `bot_self`, `bot_request_type`, `bot_request_worker`, `not_found`, `item_closed`, `not_owner`, `items_read_only`, `items_unavailable`, `item_changed`, `question_closed`, `bot_package_newer_schema`, `bot_package_duplicate_part`, `bot_package_path_not_allowed`, `secret_blocked`, `lineage_parent_changed`, `bot_offer_already_answered`, `expired`, `not_this_host`, `full`, `granted_not_added`.
- **IDE / Orchestration / Security Error Codes** (`orca_core::OrcaIdeErrorCode`): `unknown_task`, `unknown_dispatch`, `stale_dispatch`, `mailbox_is_local`, `server_mail_unsupported`, `tab_not_found`, `system_shortcut_blocked`, `approval_required`, `plugin_system_disabled`, `emulator_unsupported`, `emulator_input_blocked`, `wait_timeout`, `yaml_duplicate_key`, `yaml_size_exceeded`, `yaml_alias_limit`, `copy_budget_exceeded`, `hibernation_blocked`.
- **Canonical Constants**:
  - `BOT_AGENTS`: 22 variants (`claude`, `codex`, `grok`, `opencode`, `omp`, `pi`, `antigravity`, `cursor`, `hermes`, `copilot`, `opencode2`, `kimi`, `kiro`, `aug`, `goose`, `kilo`, `mimo-code`, `droid`, `qwen-code`, `cline`, `devin`, `codebuddy`).
  - `BOT_MEMORY_INDEX_LINE_CAP = 200`; `READD_GUARD_DAYS = 14`; `TIDY_UP_TARGET_FACTS = 60`; `TIDY_UP_MAX_NEW_FACTS = 6`; `TIDY_UP_MAX_FILED = 20`; `BOT_MAX_MEMBERS = 200`; `BOT_MAX_CONNECTOR_WORKSPACES = 8`; `WEBHOOK_HOOK_HOURLY_CAP = 200`; `WEBHOOK_ACCOUNT_HOURLY_CAP = 300`; `ORCA_YAML_MAX_BYTES = 262_144`; `WORKTREE_INCLUDE_MAX_BYTES = 2_147_483_648`; `WORKTREE_INCLUDE_MAX_ENTRIES = 50_000`; `CLOSED_TAB_TOMBSTONE_DAYS = 30`; `RUNTIME_PROTOCOL_VERSION = 3` (min 2); `E2EE_V2_KEY_SCHEDULE_BYTES = 96`.

### 2. `orca_bots` / `orca_ide` ↔ `gpui_telegram_server` & `gpui_telegram_client` (REST / WS / RPC)
- **Unified RPC Endpoint**: `POST /api/v1/orca/rpc` (`{ "method": "<ns.op>", "params": {...}, "protocol_version": 3, "caller_scope": "...", "operation_id": "..." }`) -> `{ "ok": true, "result": ... }` or `{ "ok": false, "error": { "code": "...", "message": "..." } }`.
- **Bot Fleet REST Routes**:
  - `GET/POST /api/v1/orca/bots`, `GET/PATCH/DELETE /api/v1/orca/bots/{bot_id}`, `GET /api/v1/orca/bots/templates`, `POST /api/v1/orca/bots/from-template`
  - `GET/POST /api/v1/orca/bots/{bot_id}/memory`, `POST /api/v1/orca/bots/{bot_id}/memory/forget`, `POST /api/v1/orca/bots/{bot_id}/memory/undo`, `POST /api/v1/orca/bots/{bot_id}/tidy-up`, `POST /api/v1/orca/bots/{bot_id}/tidy-up/undo`
  - `GET/POST /api/v1/orca/bots/{bot_id}/items`, `GET/PATCH /api/v1/orca/items/{item_num}`, `POST /api/v1/orca/items/{item_num}/comment`, `POST /api/v1/orca/items/{item_num}/snooze`, `GET /api/v1/orca/needs-you`
  - `GET/POST /api/v1/orca/bots/{bot_id}/look-around`, `GET/POST /api/v1/orca/bots/{bot_id}/up-next`
  - `GET/POST /api/v1/orca/groups`, `POST /api/v1/orca/groups/{group_id}/threads`, `POST /api/v1/orca/groups/{group_id}/send`, `GET /api/v1/orca/groups/{group_id}/messages`
  - `GET/POST /api/v1/orca/bots/{bot_id}/access`, `POST /api/v1/orca/bots/{bot_id}/access/decide`, `POST /api/v1/orca/bots/{bot_id}/hand-over`, `GET /api/v1/orca/bots/{bot_id}/chat-tree`
  - `POST /api/v1/orca/bots/{bot_id}/package/export`, `POST /api/v1/orca/bots/package/import`, `POST /api/v1/orca/bots/{bot_id}/share/seal`, `POST /api/v1/orca/bots/share/open`, `GET/POST /api/v1/orca/bots/import/discover`
- **Webhooks, Automations & Wake WS**:
  - `GET/POST /api/v1/orca/webhooks`, `POST /api/v1/orca/webhooks/{hook_id}/ingest` (6 adapters, zero-leak event frame, 200/300 hr caps), `GET /api/v1/orca/webhooks/{hook_id}/calls`
  - `GET/POST /api/v1/orca/automations`, `POST /api/v1/orca/automations/watch-pr`, `DELETE /api/v1/orca/automations/{id}`
  - `GET /api/v1/orca/wake` (WebSocket: content-free wake frames, close `4409` on supersede, close `1009` on oversize)
- **Orchestration & Agent IDE REST Routes**:
  - `POST/GET /api/v1/orca/orchestration/runs`, `POST /api/v1/orca/orchestration/tasks`, `POST /api/v1/orca/orchestration/workers/start`, `POST /api/v1/orca/orchestration/messages`, `GET /api/v1/orca/orchestration/mailbox`, `POST /api/v1/orca/orchestration/gates`, `POST /api/v1/orca/orchestration/gates/{id}/resolve`
  - `POST /api/v1/orca/ide/orca-yaml/parse`, `GET/POST /api/v1/orca/ide/worktrees`, `GET/POST /api/v1/orca/ide/diff-annotations`, `GET/POST /api/v1/orca/ide/vault/search`, `GET/POST /api/v1/orca/ide/plugins`, `GET/POST /api/v1/orca/ide/artifacts`, `POST /api/v1/orca/ide/computer/guard`, `GET /api/v1/orca/ide/usage`, `GET/POST /api/v1/orca/ide/hibernation/check`, `POST /api/v1/orca/ide/e2ee/handshake`

### 3. `fabushi` CLI ↔ Server / Local Runtime (`--json` Contract)
Every Orca CLI command supports `--json` (exit `0` on success with structured JSON stdout; non-zero with `{"error": {"code": "...", "message": "..."}}` on failure; exit `3` on plugin `approval_required`):
- `fabushi bot {list, create, update, delete, templates, from-template, chats, new-chat, reply, memory-forget, memory-undo, tidy-up, item-add, item-set, item-comment, item-done, item-cancel, item-link, item-list, item-show, needs-you, look-around, suggest, group-create, group-send, group-thread, access-list, access-decide, hand-over, package-export, package-import, share-seal, share-open, import-discover, chat-tree}`
- `fabushi orchestration {run-create, task-create, worker-start, send, check, ask, worker-done, worker-read, worker-release, gate-create, gate-resolve}`
- `fabushi webhooks {list, create, show, calls, check}` *(NEVER prints webhook URL or secret!)*
- `fabushi automations {list, create, edit, remove, watch-pr}`
- `fabushi worktree {parse-yaml, include-copy, list, set, cleanup, ports}`
- `fabushi ide {diff-annotate, diff-batch, vault-search, plugin-validate, plugin-install, computer-hotkey, artifact-share, hibernation-check, usage, serve, profile-state}`

### 4. `orca_bots` Loopback Tool Relay (`127.0.0.1:0`) Contract
- Accepts only `POST`, `GET`, `DELETE` (others -> `405`).
- Any request with `Origin` header OR non-loopback `Host` header -> `403`.
- Missing/invalid Bearer grant -> `401`; unauthorized bot/folder scope or revoked credential -> `404`; concurrency/rate limit exceeded -> `429`.
- Never persists static headers or OAuth tokens inside bot home folder (`SOUL.md`/`MEMORY.md`/`bot.json`/`.mcp.json`).

### 5. `gpui_telegram_desktop` ↔ `orca_bots` / `orca_ide` / `bot-browser`
- `NavDestination::OrcaBots` & `ActiveWorkspacePanel::OrcaBots` host the Bots Sidebar, Bot Chat Tree, 4-tab Bot Home (`Chat`/`Items`/`Up Next`/`Settings`), `"Needs You"` Inbox, Group Rooms, 24-Template Gallery, and `"Hand to bot…"` Dialog.
- `ActiveWorkspacePanel::OrcaIde` (and integrated overlays) host Split Anything panes, `Cmd+J` Scoped Jump Palette, `Cmd+Shift+J` Chat<->Terminal flip, Orchestration/Kanban Board, Annotate AI Diff + AI Attribution, Browser Design Mode (`bot-browser::design_mode::DesignModeCapture`), Usage Roster popover, and non-themable `SecuritySurface` modals.
- Zero modifications that delete/rename any `tdesktop` ledger-mapped file or symbol.
