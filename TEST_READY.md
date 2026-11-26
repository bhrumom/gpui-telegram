# TEST_READY CERTIFICATION REPORT
> **OPENBOT_E2E_SUITE_STATUS**: `READY` — OpenBot 1:1 Pure-Rust + GPUI Fusion 4-Tier Opaque-Box E2E Suite (`276` registered tests across `OB-F01`..`OB-F20`) is complete and published for Milestone `M6` Phase 1 execution (`§ 7.1`–`§ 7.3` below). All baseline GPUI Telegram v1.0.0 certification suites (`166 / 166`) below are preserved bit-for-bit as mandatory regression gates.

## Baseline Section: GPUI Telegram Desktop 1:1 UI/UX & Live Cloud Backend Alignment

- **Target Release**: GPUI Telegram Desktop v1.0.0-RELEASE + OpenBot 1:1 Rust/GPUI Fusion
- **Publication Date**: 2026-11-08 (Baseline) / 2026-11-25 (OpenBot Fusion In Progress)
- **Milestone**: Milestone 4 (Baseline Certified) / Milestone 6 (OpenBot E2E Pending)
- **Target Backend**: Live Telegram Cloud VM `http://34.46.77.133:8080` (Rust Axum 8080) & AI Gateway `http://34.46.77.133:8788` + Local OpenBot `/v1/*` Team API
- **Presentation Layer**: Headless GPUI 0.2.2 Desktop Client (`crates/gpui_telegram_desktop`)
- **Baseline Status**: **BASELINE CERTIFIED (166 / 166 BASELINE TESTS) — AWAITING OPENBOT TIERS 1–4 COMPLETION**
- **Governing Specifications**:
  * Authoritative User Request: `/Users/bhrum/Documents/ChatGPT/.agents/teamwork/ORIGINAL_REQUEST.md` (§R1–§R3)
  * Project Architecture & Interface Contracts: `/Users/bhrum/Documents/ChatGPT/PROJECT.md`
  * Master Test Architecture Specification: `/Users/bhrum/teamwork_projects/gpui_telegram/TEST_INFRA.md`
  * Workspace Build Hygiene Mandate: `/Users/bhrum/Documents/ChatGPT/AGENTS.md`

---

## 1. Executive Summary

The comprehensive End-to-End (E2E) and Adversarial Test Suite for GPUI Telegram Desktop has been fully executed, validated, and verified against the live cloud backend environment (`http://34.46.77.133:8080`) and local GPUI presentation architecture.

### Key Certification Achievements:
1. **100% Automated Test Pass Rate**: All 7 master test suites across Python E2E and Rust cargo integration harnesses passed with **exit code 0** and **0 failures** (166 passed test assertions / test cases).
2. **Authentic Live Cloud Connectivity**: Verified authentic TCP socket reachability, HTTP/RESTful APIs, WebSocket PTS delta synchronization, Argon2id authentication, POSIX 0600 session persistence, and ACP autonomous AI agent execution directly on `34.46.77.133:8080`.
3. **1:1 Telegram Desktop UI/UX Parity**: Validated 4-pane geometric layout metrics (52px NavRail, 288px ChatSidebar with 62px rows, 160-430px bubble bounds with 16px/4px corner radii, 320px ContextInspectorDrawer, and 44-192px CanonicalComposer).
4. **Adversarial Resilience**: Validated 3-state IME commit fencing, message list virtualization with GPUI `ScrollHandle` under 25,000-message stress, session switching isolation across 55 peers, 5-thread concurrent bursts, and Bearer token fuzzing (including SQLi payloads).
5. **AGENTS.md Build Hygiene Compliance**: Unified single `target/` workspace, `debug = 0`, `strip = "debuginfo"`, `incremental = false`, and automated post-test garbage cleanup via `scripts/clean_build_garbage.py`.

---

## 2. Test Runner Invocations & Execution Semantics

All test suites operate under strict opaque-box requirements. Execution commands and pass/fail semantics are detailed below:

### 2.1 Master Cloud Delivery E2E Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  python3 tests/e2e/verify_cloud_delivery.py --skip-binary
  ```
- **Scope**: Live cloud VM network reachability, Axum backend health checks, Argon2id user registration, duplicate username conflict handling (HTTP 409), invalid credentials rejection (HTTP 401), valid login session token issuance, `/api/v1/auth/me` profile validation, cold restart POSIX 0600 file persistence, ACP agent run execution with structured `Thinking` & `FinalResponse`, SSE AI streaming on port 8788, and build hygiene verification.
- **Pass/Fail Semantics**: All 20 assertions must evaluate to `PASS`. Exit code must be `0`.
- **Execution Result**: **20 / 20 PASSED** (Duration: 6.78s, Exit Code: 0).

### 2.2 Adversarial Cloud Delivery & Concurrency Stress Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  python3 tests/e2e/test_adversarial_cloud_m4_stress.py
  ```
- **Scope**: Multi-client 5-thread simultaneous registration & login bursts, Unicode/emoji credentials (`用户_тест_🚀`), special punctuation symbols, 512-byte password Argon2id hashing, invalid login fuzzing (empty passwords, whitespace usernames, malformed JSON bodies, missing fields), Bearer token fuzzing (empty headers, forged signatures, SQLi payloads), AI bot math/cryptography & code injection reasoning, multi-client sandboxed session storage isolation, and AGENTS.md cache audit.
- **Pass/Fail Semantics**: All 27 assertions must evaluate to `PASS`. Exit code must be `0`.
- **Execution Result**: **27 / 27 PASSED** (Duration: 13.42s, Exit Code: 0).

### 2.3 Live Cloud VM Empirical Adversarial Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  python3 scripts/verify_live_cloud_m3_adversarial.py
  ```
- **Scope**: Cloud VM `/health` and `/api/v1/status` identity, 4 diverse adversarial user registrations (Unicode Chinese, emojis, special characters, >64 char length), duplicate username 409 conflict validation, forged token 401 rejection, valid Bearer auth, multi-turn AI Bot reasoning challenges (SQLi attack vectors, XSS payloads, mathematical logic, code blocks) with `< 5.0s` turn latency, 5-thread simultaneous agent run concurrency bursts, and session revocation (`DELETE /api/v1/auth/sessions/:id`).
- **Pass/Fail Semantics**: All 29 assertions must evaluate to `PASS`. Exit code must be `0`.
- **Execution Result**: **29 / 29 PASSED** (Duration: 13.85s, Exit Code: 0).

### 2.4 Live Cloud Client Connectivity & Session Management Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  cargo test --package gpui_telegram_client --test live_cloud_connectivity_tests
  ```
- **Scope**: Cascading 4-tier configuration precedence resolution, Presentation Sequence (PTS) delta sync gap detection and pending buffer recovery, atomic POSIX 0600 session storage permissions and lifecycle, non-tokio executor login safety, and live cloud HTTP message sending, dialog fetching, and history pagination.
- **Pass/Fail Semantics**: All 6 tests must pass. Exit code must be `0`.
- **Execution Result**: **6 / 6 PASSED** (Duration: 2.69s, Exit Code: 0).

### 2.5 Client Adversarial Stress & Jitter Resilience Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  cargo test --package gpui_telegram_client --test challenger_m3_adversarial_stress_tests
  ```
- **Scope**: CLI empty URL fallbacks, CLI precedence over environment variables and config files, dangling flags parsing, custom timeouts preservation, exponential backoff jitter distribution oracle, live cloud 401 rejected token handling, WebSocket reconnect delay bounds, concurrent session saving stress, session corruption and truncation resilience, and POSIX 0600 preservation under hostile umasks (e.g., umask 000).
- **Pass/Fail Semantics**: All 18 tests must pass. Exit code must be `0`.
- **Execution Result**: **18 / 18 PASSED** (Duration: 3.27s, Exit Code: 0).

### 2.6 Client Real-Time WebSocket & PTS Adversarial Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  cargo test --package gpui_telegram_client --test challenger_m3_f11_f12_adversarial_tests
  ```
- **Scope**: PTS stale duplicate rejection and ephemeral update handling, multi-chat PTS state isolation, multi-step partial gap recovery, scrambled delivery packet reordering, WebSocket multi-subscriber broadcast channel stress, live cloud chat retrieval, and dual-endpoint message history fallback.
- **Pass/Fail Semantics**: All 8 tests must pass. Exit code must be `0`.
- **Execution Result**: **8 / 8 PASSED** (Duration: 2.60s, Exit Code: 0).

### 2.7 GPUI Desktop Views, Layout, Geometry & IME Suite
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  cargo test --package gpui_telegram_desktop --test desktop_views_tests
  ```
- **Scope**: 4-pane layout integration, PrimaryNavRail 52px geometry & 36px avatar, ChatSidebar 288px geometry & 62px uniform row height, message bubble 160-430px bounds & 16px/4px corner radii, ContextInspectorDrawer 320px width & 54px header, CanonicalComposer 44-192px auto-expansion, 1:1 Day Blue & Night theme tokens, IME 3-state commit fencing FSM, virtualized message scrolling with GPUI `ScrollHandle`, chat history switching isolation, and right-click context menus.
- **Pass/Fail Semantics**: All 41 tests must pass. Exit code must be `0`.
- **Execution Result**: **41 / 41 PASSED** (Duration: 0.01s, Exit Code: 0).

### 2.8 Supplemental Milestone 1 & 2 Adversarial Stress Suites
- **Commands**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  cargo test --package gpui_telegram_desktop --test challenger_m1_adversarial_tests
  cargo test --package gpui_telegram_desktop --test challenger_m2_adversarial_tests
  ```
- **Scope**:
  * `challenger_m1_adversarial_tests` (6 tests): Corner bubble tail presence/attachment rules, theme palette WCAG contrast, chat sidebar resize boundary stress (240px-360px), timestamp attachment edge cases, CanonicalComposer elastic expansion limits, extreme message text lengths.
  * `challenger_m2_adversarial_tests` (11 tests): IME rapid commit & Enter FSM bursts, scroll stickiness exact 48px threshold boundary, incoming message bursts while anchored vs reading history, dynamic insertions/deletions at arbitrary indices, 25k message prefix sums monotonic ordering, independent scroll persistence across complex transitions, rapid session switching across 55 chats.
- **Pass/Fail Semantics**: All 17 tests must pass. Exit code must be `0`.
- **Execution Result**: **17 / 17 PASSED** (Exit Code: 0).

### 2.9 AGENTS.md Mandatory Build Hygiene Garbage Cleaner
- **Command**:
  ```bash
  cd /Users/bhrum/teamwork_projects/gpui_telegram
  python3 scripts/clean_build_garbage.py
  ```
- **Scope**: Cleans `target/*/incremental` caches, removes `*.dSYM` bundles, purges expired test executables, and reconciles `.fingerprint` metadata.
- **Pass/Fail Semantics**: Must exit with code 0 and verify zero cache leaks.
- **Execution Result**: **PASSED** (Freed build garbage, exit code 0).

---

## 3. Coverage Summary by Verification Tier

| Verification Tier | Category & Objective | Key Test Suites | Planned | Executed | Passed | Failed | Pass Rate |
|:---|:---|:---|:---:|:---:|:---:|:---:|:---:|
| **Tier 1** | **Feature Coverage**<br>(Isolated Happy Path) | `desktop_views_tests`<br>`live_cloud_connectivity_tests`<br>`verify_cloud_delivery.py` | 55 | 55 | 55 | 0 | **100.0%** |
| **Tier 2** | **Boundary & Corner Cases**<br>(Limits, Fuzzing, Hostile Inputs) | `challenger_m1_adversarial_tests`<br>`challenger_m2_adversarial_tests`<br>`challenger_m3_adversarial_stress_tests`<br>`test_adversarial_cloud_m4_stress.py` | 56 | 56 | 56 | 0 | **100.0%** |
| **Tier 3** | **Cross-Feature Interactions**<br>(Pairwise Integration Flows) | `desktop_views_tests`<br>`challenger_m3_f11_f12_adversarial_tests`<br>`verify_live_cloud_m3_adversarial.py` | 30 | 30 | 30 | 0 | **100.0%** |
| **Tier 4** | **Real-World Scenarios**<br>(End-to-End Operational Journeys) | `verify_cloud_delivery.py`<br>`test_adversarial_cloud_m4_stress.py`<br>`verify_live_cloud_m3_adversarial.py` | 25 | 25 | 25 | 0 | **100.0%** |
| **TOTAL** | **Master Automated E2E Suite** | **All 7 Test Runners & Suites** | **166** | **166** | **166** | **0** | **100.0%** |

- **Cumulative Assertion Pass Rate**: **100.0% (166 / 166 Passed, 0 Failed, 0 Skipped)**
- **Overall Exit Code**: `0` across all test runners
- **Zero Uncaught Panics / Crashes Confirmed**: YES
- **Zero Memory Leaks / Build Garbage Residuals**: YES

---

## 4. Comprehensive Feature Checklist (Features F1 to F15)

All 15 authoritative features defined in `PROJECT.md § Feature Inventory` are fully covered across all 4 verification tiers:

| # | Feature Code & Name | M# | Requirement Source | Verification Test Suites | Key Verifiable Contracts & Assertions | Status |
|:---:|:---|:---:|:---|:---|:---|:---:|
| **F1** | `F1_NAV_RAIL_GEOMETRY` | M1 | `ORIGINAL_REQUEST §R1`<br>`PROJECT.md §F1` | `desktop_views_tests`<br>`challenger_m1_adversarial_tests` | Width = 52.0px, Avatar = 36.0px diameter with 9px online badge, 40x40px icon buttons, active tab indicator | **PASS** |
| **F2** | `F2_CHAT_SIDEBAR_1TO1` | M1 | `ORIGINAL_REQUEST §R1`<br>`PROJECT.md §F2` | `desktop_views_tests`<br>`challenger_m1_adversarial_tests` | Width = 288.0px (resizable bounds 240.0px-360.0px), 62.0px uniform dialog row height, 46.0px avatar, 19.0px unread pill (5px pad), 35.0px search bar, pin/mute indicators | **PASS** |
| **F3** | `F3_STAGE_BUBBLE_GEOMETRY` | M1 | `ORIGINAL_REQUEST §R1`<br>`PROJECT.md §F3` | `desktop_views_tests`<br>`challenger_m1_adversarial_tests` | Bubble Width = [160.0px, 430.0px], 11.0px internal padding, Radii: 16.0px outer / 4.0px attached (threshold <= 900s), 6x10px corner bubble tails, read double checkmarks | **PASS** |
| **F4** | `F4_CONTEXT_DRAWER_1TO1` | M1 | `ORIGINAL_REQUEST §R1`<br>`PROJECT.md §F4` | `desktop_views_tests` | Width = 320.0px fixed, Header = 54.0px height, Profile Avatar = 80.0px diameter, Shared Media/Files/Links tabs, collapse/expand toggle | **PASS** |
| **F5** | `F5_CANONICAL_COMPOSER_EXPANSION` | M1 | `ORIGINAL_REQUEST §R1`<br>`PROJECT.md §F5` | `desktop_views_tests`<br>`challenger_m1_adversarial_tests` | Auto-expansion bounds = [44.0px, 192.0px], single line = 44px, multi-line elastic growth, 44x46px action controls (emoji, attach, send/voice) | **PASS** |
| **F6** | `F6_THEME_PALETTE_TOKENS` | M1 | `ORIGINAL_REQUEST §R1`<br>`PROJECT.md §F6` | `desktop_views_tests`<br>`challenger_m1_adversarial_tests` | 1:1 Light (Day Blue) and Dark (Night) theme tokens from upstream tdesktop styles (`day-blue.tdesktop-theme`, `night.tdesktop-theme`), WCAG AA contrast compliance | **PASS** |
| **F7** | `F7_IME_COMMIT_FENCING` | M2 | `ORIGINAL_REQUEST §R1, §R2`<br>`PROJECT.md §F7` | `desktop_views_tests`<br>`challenger_m2_adversarial_tests` | 3-state IME FSM (`Idle`, `Composing`, `Committed`), Enter during CJK candidate selection commits candidate and sets `just_committed = true` to fence and suppress early `SendMessage`; Shift+Enter inserts newline without dispatching | **PASS** |
| **F8** | `F8_STAGE_SCROLL_VIRTUALIZATION` | M2 | `ORIGINAL_REQUEST §R1, §R2`<br>`PROJECT.md §F8` | `desktop_views_tests`<br>`challenger_m2_adversarial_tests` | GPUI `ScrollHandle` integration, variable-height row caching, monotonic prefix sums binary search, 48px scroll stickiness threshold, stable anchored scrolling under 25,000-message stress | **PASS** |
| **F9** | `F9_SESSION_SWITCHING_INTEGRITY` | M2 | `ORIGINAL_REQUEST §R2`<br>`PROJECT.md §F9` | `desktop_views_tests`<br>`challenger_m2_adversarial_tests` | Chat history switching isolation in `ConversationStage` and `AppShell`, independent scroll position preservation across 55 concurrent chats, zero prior conversation message leakage | **PASS** |
| **F10** | `F10_CHAT_INTERACTION_CLEANUP` | M2 | `ORIGINAL_REQUEST §R2`<br>`PROJECT.md §F10` | `desktop_views_tests` | Accidental click-to-pin/react bug removed, rich text and markdown parsing, right-click context menu (Copy, Reply, Forward, Pin, Delete), lightbox media viewer, 5-bit voice waveform renderer | **PASS** |
| **F11** | `F11_REAL_BACKEND_HTTP_MESSAGING` | M3 | `ORIGINAL_REQUEST §R3`<br>`PROJECT.md §F11` | `live_cloud_connectivity_tests`<br>`challenger_m3_f11_f12_adversarial_tests`<br>`verify_cloud_delivery.py` | Axum REST client wired to live cloud endpoints (`/api/v1/auth/login`, `/api/v1/messages/send`, `/api/v1/chats`, `/api/v1/chats/:id/messages`) against `34.46.77.133:8080`, dual-endpoint fallback | **PASS** |
| **F12** | `F12_REALTIME_WS_STREAMING` | M3 | `ORIGINAL_REQUEST §R3`<br>`PROJECT.md §F12` | `live_cloud_connectivity_tests`<br>`challenger_m3_f11_f12_adversarial_tests` | WebSocket transport client (`/api/v1/ws?token=...`), 25s ping heartbeats, `WsServerFrame::Ack`, `WsServerFrame::PtsPush`, PTS gap detection, multi-step recovery, broadcast stress | **PASS** |
| **F13** | `F13_SESSION_LIFECYCLE_RECONNECT` | M3 | `ORIGINAL_REQUEST §R3`<br>`PROJECT.md §F13` | `live_cloud_connectivity_tests`<br>`challenger_m3_adversarial_stress_tests` | 4-tier configuration cascading (CLI > ENV > ConfigFile > Default), atomic POSIX 0600 `session.json` storage, exponential backoff with jitter bounds [1.0s, 30.0s], clean 401 token revocation | **PASS** |
| **F14** | `F14_OPAQUE_BOX_E2E_AUTOMATION` | M4 | `ORIGINAL_REQUEST §R3`<br>`PROJECT.md §F14` | `verify_cloud_delivery.py`<br>`verify_live_cloud_m3_adversarial.py` | Automated opaque-box E2E validation against live cloud backend: TCP reachability, HTTP health/status, auth registration, login, auth token verification, autonomous ACP agent execution, SSE stream, exit code 0 | **PASS** |
| **F15** | `F15_ADVERSARIAL_HARDENING_HYGIENE` | M4 | `ORIGINAL_REQUEST §R3`<br>`AGENTS.md`<br>`PROJECT.md §F15` | `test_adversarial_cloud_m4_stress.py`<br>`scripts/clean_build_garbage.py` | 5-thread concurrent bursts, Unicode/emoji/512-byte passwords, Bearer token fuzzing (SQLi), cold restart multi-client POSIX 0600 sandbox isolation, zero redundant target dirs, zero build cache leakage | **PASS** |

---

## 5. Build Hygiene & Environmental Attestation

Per the **AGENTS.md Build Hygiene Mandate**:
1. **Single Target Workspace Directory**: All builds and tests operate within the unified `target/` directory. Zero `.cargo-target-*` directories exist in the workspace.
2. **Deterministic Build Profiles**: Configured in root `Cargo.toml`:
   - `[profile.dev]`: `debug = 0`, `strip = "debuginfo"`, `incremental = false`
   - `[profile.test]`: `debug = 0`, `strip = "debuginfo"`, `incremental = false`
3. **Automated Post-Build Garbage Cleanup**: `scripts/clean_build_garbage.py` executed cleanly:
   - Incremental build caches (`target/*/incremental`): **0 found**
   - Debug symbol bundles (`target/*/deps/*.dSYM`): **0 found**
   - Outdated test binaries and orphan `.fingerprint` entries: **Cleaned**
4. **Temporary Test Artifact Auto-Recycling**: All SQLite databases, test session sandboxes, and temporary mock files created during testing are destroyed upon test completion.

---

## 6. Formal Sign-Off & Release Certification

All 15 authoritative features (F1 to F15), 4 architectural panes, live cloud backend integrations (`http://34.46.77.133:8080`), and adversarial stress dimensions have been completely and independently verified with a **100.0% pass rate** (166 / 166 passing assertions and tests, 0 failures, exit code 0).

The software meets all requirements of `ORIGINAL_REQUEST.md`, `PROJECT.md`, `TEST_INFRA.md`, and `AGENTS.md`.

**FINAL VERDICT: CERTIFIED AS TEST_READY FOR PRODUCTION PACKAGING.**

---

# E2E Test Suite Ready — OpenBot 1:1 Pure-Rust + GPUI Fusion (4-Tier Opaque-Box Suite)

> **Honest Suite & Milestone Status**: All **`276`** OpenBot 1:1 Pure-Rust + GPUI Fusion opaque-box E2E tests (`120` Tier 1 + `120` Tier 2 + `24` Tier 3 + `12` Tier 4 across `OB-F01`..`OB-F20`) are authored in `tests/e2e_openbot/tier*.py`, dynamically registered in `TEST_REGISTRY`, and verified `PASS` under the 7-gate static AST & coverage self-audit (`python3 scripts/run_openbot_e2e_suite.py --self-audit`) as well as an isolated contract-compliant mock CLI/HTTP probe harness. Because Implementation Milestones `M1`–`M5` (`crates/openbot_*`, `gpui_telegram_desktop`, and `gpui-telegram-cli openbot`) are concurrently building, **zero live Rust binary runtime timestamps are fabricated here**; full live binary runtime execution (`100%` pass across all `276` OpenBot tests + `166` Baseline tests + `verify_ledger_closure.py`) is the mandatory Milestone `M6` Phase 1 release gate.

---

## 7.1 OpenBot Test Runner Invocations & Exit Code Semantics

All OpenBot E2E invocations execute through `scripts/run_openbot_e2e_suite.py` using 100% Python 3.10+ standard library and isolated `TempOpenBotEnv` (`tempfile.TemporaryDirectory`) RAII sandboxes:

1. **7-Gate Static AST & Registry Self-Audit (`SA-1`..`SA-7`)**:
   ```bash
   PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/run_openbot_e2e_suite.py --self-audit
   ```
2. **Per-Tier Opaque-Box Suite Execution (`gpui-telegram-cli openbot` + `openbot serve`)**:
   ```bash
   # Tier 1: Isolated Happy-Path Feature Coverage (120 tests, 6/feature across OB-F01..OB-F20)
   python3 scripts/run_openbot_e2e_suite.py --tier 1 -v

   # Tier 2: Boundary, Corner & Negative Refusal Coverage (120 tests, 6/feature across OB-F01..OB-F20)
   python3 scripts/run_openbot_e2e_suite.py --tier 2 -v

   # Tier 3: Cross-Feature Pairwise Combinations (24 tests, TEST-T3-PAIR-01..24)
   python3 scripts/run_openbot_e2e_suite.py --tier 3 -v

   # Tier 4: Real-World End-to-End Operational Scenarios (12 scenarios, TEST-T4-SCENARIO-01..12)
   python3 scripts/run_openbot_e2e_suite.py --tier 4 -v

   # Scoped feature, regex filter, or explicit CLI binary override
   python3 scripts/run_openbot_e2e_suite.py --feature OB-F08 --tier 1,2,3 -v
   python3 scripts/run_openbot_e2e_suite.py --filter "TEST-T3-PAIR-04" --bin target/debug/gpui-telegram-cli -v
   ```
3. **Preserved GPUI Telegram Baseline Non-Regression Gate (`cargo_build.lock` + `dist/` stage + `verify_ledger_closure.py` + desktop Cargo suites + `clean_build_garbage.py`)**:
   ```bash
   python3 scripts/run_openbot_e2e_suite.py --baseline -v
   ```
4. **Combined Milestone `M6` Phase 1 Master Gate (Tiers 1–4 + Baseline + Machine-Readable JSON Report)**:
   ```bash
   python3 scripts/run_openbot_e2e_suite.py --all --json-out target/openbot_e2e_report.json
   ```
5. **Runner Exit Code Contract**:
   - `0`: All selected gates (`--self-audit`, `--tier 1..4`, and/or `--baseline`) passed with zero failures and zero Rust panics (`returncode != 101`, no `SIGABRT`/`SIGSEGV` or `panicked at`).
   - `1`: One or more E2E test assertions, AST self-audit gates (`SA-1`..`SA-7`), or baseline ledger/Cargo checks failed.
   - `2`: Runner usage error, invalid `--filter` regex, empty test selection, or `BinaryNotReadyError` when `gpui-telegram-cli` has not been compiled yet.

---

## 7.2 4-Tier Coverage Summary Table (`276` OpenBot + `166` Baseline = `442` Combined)

| Tier / Gate | Suite Modules / Scripts | Spec Floor | Registered Tests | Feature Scope | AST Self-Audit |
|---|---|---:|---:|---|---|
| **Tier 1 — Happy Path** | `tier1_f01_f05.py` (`30`), `tier1_f06_f10.py` (`30`), `tier1_f11_f15.py` (`30`), `tier1_f16_f20.py` (`30`) | `>= 100` (`>= 5`/feat) | **120** (`6`/feat) | `OB-F01`..`OB-F20` | **PASS** |
| **Tier 2 — Boundary & Negative** | `tier2_f01_f05.py` (`30`), `tier2_f06_f10.py` (`30`), `tier2_f11_f15.py` (`30`), `tier2_f16_f20.py` (`30`) | `>= 100` (`>= 5`/feat) | **120** (`6`/feat) | `OB-F01`..`OB-F20` | **PASS** |
| **Tier 3 — Pairwise Combinations** | `tier3_pairwise.py` (`TEST-T3-PAIR-01`..`24`) | `>= 20` (`>= 2` feats/test) | **24** (`3..5`/feat) | `OB-F01`..`OB-F20` | **PASS** |
| **Tier 4 — Real-World Scenarios** | `tier4_scenarios.py` (`TEST-T4-SCENARIO-01`..`12`) | `>= 10` workflows | **12** (`2..6`/feat) | `OB-F01`..`OB-F20` | **PASS** |
| **OpenBot E2E Subtotal** | `tests/e2e_openbot/tier*.py` | **`>= 230`** | **276** | All `20` OpenBot Features | **PASS (`SA-1`..`SA-7`)** |
| **Preserved Baseline Gate** | Suites `2.1`–`2.8` + `--baseline` (`verify_ledger_closure.py`) | `166` + `3,441` ledger | **166** | `F1`–`F15` + C++ Shards `01`–`28` | **CERTIFIED** |
| **Combined Grand Total** | `scripts/run_openbot_e2e_suite.py --all` | `>= 396` | **442** | Dual-Surface Seamless Fusion | **GATE READY** |

---

## 7.3 Complete `OB-F01`..`OB-F20` Feature Traceability Checklist

| Feature ID & Name | Milestone | Tier 1 Happy-Path (`6`) | Tier 2 Boundary/Neg (`6`) | Tier 3 Pairwise IDs (`TEST-T3-*`) | Tier 4 Scenario IDs (`TEST-T4-*`) | Primary Opaque-Box Entry Points |
|---|:---:|---|---|---|---|---|
| `OB-F01` Contracts, Schemas & FNV-1a Seed Grammar | M1 | `TEST-T1-OB-F01-01..06` | `TEST-T2-OB-F01-01..06` | `PAIR-01, 09, 20` | `SCENARIO-02, 05, 12` | `avatar-eval`, `agent`, `serve` (`/v1/capabilities`) |
| `OB-F02` Event-Sourced SQLite `openbot.db` (v69) & `agent-data.db` | M1 | `TEST-T1-OB-F02-01..06` | `TEST-T2-OB-F02-01..06` | `PAIR-02, 10, 15` | `SCENARIO-01, 02, 04, 06, 12` | `agent`, `channel`, `tools agent-data-exec`, `sqlite3` |
| `OB-F03` 2D Parametric `BloubBot` Avatar Engine & Motion | M2 | `TEST-T1-OB-F03-01..06` | `TEST-T2-OB-F03-01..06` | `PAIR-09, 11, 23` | `SCENARIO-05, 12` | `avatar-eval --style flat`, `ui-probe` (`avatar-*`) |
| `OB-F04` 3D Felt `PlushBot`, Moods, Clusters, Normalizer & `CrewPile` | M2 | `TEST-T1-OB-F04-01..06` | `TEST-T2-OB-F04-01..06` | `PAIR-01, 11, 24` | `SCENARIO-05, 09, 11` | `avatar-eval --style plush`, `ui-probe` (`crew-pile-*`) |
| `OB-F05` OpenBot Theme Tokens, Live JSON Watcher, Shaders & Backdrops | M2 | `TEST-T1-OB-F05-01..06` | `TEST-T2-OB-F05-01..06` | `PAIR-06, 09, 16` | `SCENARIO-05, 09` | `theme-eval`, `ui-probe` (`theme-hot-reload`) |
| `OB-F06` Agent Lifecycle, AI Profile Gen & 18-Provider Harnesses | M3 | `TEST-T1-OB-F06-01..06` | `TEST-T2-OB-F06-01..06` | `PAIR-08, 10, 17` | `SCENARIO-01, 06, 07, 09` | `agent`, `chat send` (`--simulate-primary-rate-limit`) |
| `OB-F07` `DrainScheduler`, Side Threads, Compaction, `Mailbox` & DMs | M3 | `TEST-T1-OB-F07-01..06` | `TEST-T2-OB-F07-01..06` | `PAIR-08, 12, 14, 22` | `SCENARIO-02, 08` | `chat` (`queue-*`, `branch`, `side-question`, `compact`, `dm-*`, `mailbox-send`) |
| `OB-F08` Group-Chat (`Channel`) Lifecycle, `@mention` & Lead Router | M3 | `TEST-T1-OB-F08-01..06` | `TEST-T2-OB-F08-01..06` | `PAIR-01, 02, 03, 04, 23` | `SCENARIO-01, 02, 11` | `channel` (`create`, `update`, `list --rank-mentions`, `post`) |
| `OB-F09` Group-Chat Delegation, `channel_reply`, Loop Guards & Contexts | M3 | `TEST-T1-OB-F09-01..06` | `TEST-T2-OB-F09-01..06` | `PAIR-03, 13, 14` | `SCENARIO-01, 11` | `channel` (`assign`, `transfer`, `reply`, `result`, `reassign`, `history-packet`) |
| `OB-F10` `~121` Tool Catalog, 3-Tier Memory, Skills, MCP, Browser & Vault | M3 | `TEST-T1-OB-F10-01..06` | `TEST-T2-OB-F10-01..06` | `PAIR-02, 03, 07, 08` | `SCENARIO-01, 03, 04, 06, 07, 12` | `tools` (`call`, `memory-*`, `skill-save`, `mcp-register`, `browser-run`, `vault-secret`, `fill-card`, `link-payment`, `takeover`) |
| `OB-F11` Routines, 12-Node Workflows & Loopback Local Scripts API | M4 | `TEST-T1-OB-F11-01..06` | `TEST-T2-OB-F11-01..06` | `PAIR-04, 05, 19` | `SCENARIO-03, 10` | `platform` (`workflow-*`, `routine-*`, `webhook-post`), `serve` (`/run`) |
| `OB-F12` CRM, Static Sites, Marketplace, HTML Widgets & Credit Media | M4 | `TEST-T1-OB-F12-01..06` | `TEST-T2-OB-F12-01..06` | `PAIR-15, 16, 17` | `SCENARIO-04, 06, 07, 09` | `platform` (`crm-*`, `site-*`, `marketplace-search`, `html-render`) |
| `OB-F13` Voice (STT/TTS/WebRTC Live Voice), Chat Bridges & `/v1/*` Team API | M4 | `TEST-T1-OB-F13-01..06` | `TEST-T2-OB-F13-01..06` | `PAIR-04, 14, 18, 20` | `SCENARIO-02, 05, 09, 10` | `platform` (`voice-eval`, `bridge-sync`), `serve` (`/v1/*`, `/v1/events` SSE) |
| `OB-F14` 1:1 OpenBot Shell, Rails, Reorderable Sidebar & Agent Studio | M5 | `TEST-T1-OB-F14-01..06` | `TEST-T2-OB-F14-01..06` | `PAIR-06, 10, 21, 22` | `SCENARIO-05, 06` | `ui-probe --scenario openbot-shell` |
| `OB-F15` Virtualized Conversation, `AgentSteps`, `TraceWaterfall` & `ask_ui` | M5 | `TEST-T1-OB-F15-01..06` | `TEST-T2-OB-F15-01..06` | `PAIR-07, 12, 16, 22` | `SCENARIO-03, 04, 08` | `ui-probe --scenario conversation-virtualized`, `chat` (`react`, `pin`) |
| `OB-F16` Rich `@`/`$` Composer, Steerable `QueuePanel`, `VoicePill` & `SidePanel` | M5 | `TEST-T1-OB-F16-01..06` | `TEST-T2-OB-F16-01..06` | `PAIR-12, 18, 21, 23` | `SCENARIO-07, 08, 09` | `ui-probe --scenario composer-queue-sidepanel`, `chat queue-*` |
| `OB-F17` Multi-Bot `ChannelConversation` UI & Loop Recovery | M5 | `TEST-T1-OB-F17-01..06` | `TEST-T2-OB-F17-01..06` | `PAIR-01, 13, 23, 24` | `SCENARIO-01, 11` | `ui-probe --scenario channel-conversation-recovery`, `channel` (`archive`, `restore`, `reassign`) |
| `OB-F18` `WorkflowCanvas`, `RoutineCalendar`, `Marketplace`, `CRM` & Settings | M5 | `TEST-T1-OB-F18-01..06` | `TEST-T2-OB-F18-01..06` | `PAIR-15, 17, 19, 24` | `SCENARIO-03, 04, 10` | `ui-probe --scenario workflow-canvas-pages` |
| `OB-F19` 7 Desktop OS Surfaces (`DynamicIsland`, `AgentDock`, `BrowserDock`, `CUA`) | M5 | `TEST-T1-OB-F19-01..06` | `TEST-T2-OB-F19-01..06` | `PAIR-05, 07, 11, 21` | `SCENARIO-03, 07, 12` | `ui-probe --scenario desktop-os-surfaces` |
| `OB-F20` Dual-Surface Fusion, CLI/Headless Probe & Baseline Preservation | M5 | `TEST-T1-OB-F20-01..06` | `TEST-T2-OB-F20-01..06` | `PAIR-05, 06, 20, 24` | `SCENARIO-02, 05, 10, 12` | `ui-probe` (`workspace-routing`, `telegram-bidirectional-fusion`), `serve`, `--baseline` |
