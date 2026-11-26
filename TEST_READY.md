# E2E Test Suite Ready — Orca Pure-Rust Absorption (`gpui-telegram` / Fabushi v1.7.0)

- **Status**: **READY FOR M6 EXECUTION** (`5/5` Holistic Track-Level Verification Gate R2 PASS — 2/2 Reviewers `APPROVE`, 2/2 Challengers `CONFIRM`, Forensic Auditor `CLEAN`)
- **Published**: `2026-11-25T16:42:00Z`
- **Infrastructure Spec**: `/home/antigravity/bhrumom/gpui-telegram/TEST_INFRA.md`
- **Suite Root**: `/home/antigravity/bhrumom/gpui-telegram/tests/orca_e2e/` (`40` files: `10` core harness + `30` Tier 1–4 test modules; pure Python 3.12 stdlib + `pytest` core; zero third-party deps; zero edits to `Cargo.toml`/`crates/**`/`.github/**`)
- **Authoritative Requirements**: `.agents/teamwork/ORIGINAL_REQUEST.md`, `PROJECT.md` (`F01`–`F24`, Interface Contracts §§1–5), `orca_core_spec.md` (`SM1-01..SM1-14`), `orca_bots_and_designs_spec.md` (`SM2-01..SM2-26`), `gpui_telegram_survey.md`

---

## 1. Canonical Runner Commands

Always run via `tests/orca_e2e/run_e2e.py` (which resolves `/opt/pipx/bin/pytest` with fallback to `/usr/bin/python3 -m pytest`, enforces `PYTHONDONTWRITEBYTECODE=1`, and loads `-c tests/orca_e2e/pytest.ini` with `-p no:cacheprovider --strict-markers` to prevent untracked `.pytest_cache/` or `__pycache__/` pollution):

```bash
# 1. Verify full collection across all Tiers 1–4 + Harness Self-Checks (353 total)
PYTHONDONTWRITEBYTECODE=1 /opt/pipx/bin/pytest -c tests/orca_e2e/pytest.ini --collect-only tests/orca_e2e

# 2. Run Harness Self-Integrity Checks (23 meta-tests, excluded from Tiers 1–4)
PYTHONWARNINGS=error::ResourceWarning /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier self -v

# 3. Build SUT binaries (M6 execution prerequisite — always follow with clean_build_garbage.py)
cargo build --bin gpui_telegram_server --bin fabushi && python3 scripts/clean_build_garbage.py

# 4. Run E2E Tiers sequentially in M6 (Tier 1 -> Tier 2 -> Tier 3 -> Tier 4, or all)
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 1 -v
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 2 -v
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 3 -v
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 4 -v
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier all -v

# Optional: target a specific feature or externally running server
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 1 -m f01 -v
/usr/bin/python3 tests/orca_e2e/run_e2e.py --tier all --server-url http://127.0.0.1:18080 -v
```

---

## 2. Verified Tier Counts & Thresholds

Independently verified on **both** `/opt/pipx/bin/pytest` (`pytest 9.2.0`) and `/usr/bin/python3 -m pytest` (`pytest 7.4.4`) with `-W error` (`0` warnings):

| Tier | Pytest Marker / Flag | Minimum Required | Delivered & Verified | Status |
|---|---|---:|---:|---|
| **Tier 1 — Feature Coverage** | `@pytest.mark.tier1` (`--tier 1`) | >= 120 (>= 5/feature) | **144** (exact **6**/feature across `F01`–`F24`) | **PASS** |
| **Tier 2 — Boundary & Corner Cases** | `@pytest.mark.tier2` (`--tier 2`) | >= 120 (>= 5/feature, 40/40 edge cases) | **144** (exact **6**/feature across `F01`–`F24`, **40/40** `SM1-01..14` + `SM2-01..26`) | **PASS** |
| **Tier 3 — Cross-Feature Pairwise** | `@pytest.mark.tier3` (`--tier 3`) | >= 24 | **28** (all 28 combine `>= 2` features; **24/24** features covered) | **PASS** |
| **Tier 4 — Real-World Scenarios** | `@pytest.mark.tier4` (`--tier 4`) | >= 12 | **14** (**24/24** features covered; full multi-step workflows) | **PASS** |
| **Tiers 1–4 Grand Total** | `--tier all` | **>= 276** | **330** | **PASS** |
| **Harness Self-Checks** *(meta)* | `@pytest.mark.harness_self_check` (`--tier self`) | *(excluded from T1–T4)* | **23** (`23/23` PASS live with `ResourceWarning=error`) | **PASS** |
| **Total Collected** | `tests/orca_e2e` | — | **353** (`330` T1–T4 + `23` self-checks; `0` overlap) | **PASS** |

---

## 3. Complete 24-Feature Coverage Checklist (`F01`–`F24`)

| Feature | Marker | Area Suite | T1 | T2 | T3 | T4 | Upstream Edge Cases Verified |
|---|---|---|---:|---:|---:|---:|---|
| **F01** Bot Identity, 22-Agent Schema & CAS Avatars | `f01` | `area_bots_memory_items` (E1) | 6 | 6 | 1 | 1 | `SM2-01`, `SM2-02`, `SM2-03`, `SM2-04`, `SM2-25` |
| **F02** Bot Home, Authority Preamble, First-Run & `NO_REPLY` | `f02` | `area_bots_memory_items` (E1) | 6 | 6 | 2 | 1 | `SM2-08`, `SM2-16` |
| **F03** Memory Provenance, Protected Rules, Journal, Tidy-Up & 14d Re-Add Guard | `f03` | `area_bots_memory_items` (E1) | 6 | 6 | 3 | 2 | `SM2-05`, `SM2-06`, `SM2-07` |
| **F04** Event-Sourced Bot Items, Multi-Decision Ask Cards & `"Needs You"` | `f04` | `area_bots_memory_items` (E1) | 6 | 6 | 4 | 1 | `SM2-04`, `SM2-09`, `SM2-10` |
| **F05** Proactive Read-Only `"Look Around"` -> `"Up Next"` | `f05` | `area_bots_memory_items` (E1) | 6 | 6 | 2 | 1 | `SM2-11` |
| **F06** Multi-Bot Group Rooms, Sub-Threads, Peer Mail & Remote Bots | `f06` | `area_connectors_relay_groups` (E2) | 6 | 6 | 2 | 1 | `SM2-04`, `SM2-21` |
| **F07** 24 Templates, 13 Skills, Packages, E2EE Share, Import & Hand-Over | `f07` | `area_bots_memory_items` (E1) | 6 | 6 | 3 | 3 | `SM2-19`, `SM2-20` |
| **F08** Loopback Tool Relay, Host OAuth 2.1 & Shared MCP Pool | `f08` | `area_connectors_relay_groups` (E2) | 6 | 6 | 3 | 2 | `SM2-12`, `SM2-13`, `SM2-14` |
| **F09** External Connectors, `ActsFor`, Access, Slack Mirror, Triggers, Webhooks, Waits & Wake WS | `f09` | `area_connectors_relay_groups` (E2) | 6 | 6 | 3 | 3 | `SM2-15`, `SM2-17`, `SM2-18`, `SM2-26` |
| **F10** Strict `orca.yaml`, `.worktreeinclude`, Folder Workspaces & Parallel Worktrees | `f10` | `area_orchestration_worktrees_pty` (E3) | 6 | 6 | 3 | 2 | `SM1-01`, `SM1-02`, `SM1-14`, `SM2-22` |
| **F11** Durable SQLite Multi-Agent Orchestration & Cross-Host Federation | `f11` | `area_orchestration_worktrees_pty` (E3) | 6 | 6 | 5 | 2 | `SM1-10` |
| **F12** Extended 22+ CLI Agents, Event-Sourced Chat Journal & Chat<->Terminal | `f12` | `area_orchestration_worktrees_pty` (E3) | 6 | 6 | 3 | 1 | *(Chat<->Terminal lineage & idempotency)* |
| **F13** 4-State Agent Status Engine, Hook Server, Rules & Safe Hibernation | `f13` | `area_orchestration_worktrees_pty` (E3) | 6 | 6 | 2 | 1 | `SM1-09`, `SM2-23` |
| **F14** Detached PTY Persistence, Cold-Park, Credit/ACK Multiplex & Tombstones | `f14` | `area_orchestration_worktrees_pty` (E3) | 6 | 6 | 2 | 1 | `SM1-06`, `SM1-07`, `SM1-08` |
| **F15** Multi-Account Hot-Swap & 9-Provider Usage / Rate-Limit Roster | `f15` | `area_orchestration_worktrees_pty` (E3) | 6 | 6 | 2 | 1 | *(corrupt config fallback & 80% chip)* |
| **F16** ProfileState SQLite, AtomicSnapshotWriter, Keyring & AI Vault FTS | `f16` | `area_vault_review_browser_plugins_ui` (E4) | 6 | 6 | 2 | 2 | `SM1-03`, `SM1-04`, `SM1-05`, `SM1-13` |
| **F17** Annotate AI Diff, Line-Level AI Attribution, Source Control & Tasks Hub | `f17` | `area_vault_review_browser_plugins_ui` (E4) | 6 | 6 | 3 | 1 | *(shift tracking & zero git pollution)* |
| **F18** Embedded Browser Design Mode, Safe Computer Use & Mobile Emulator | `f18` | `area_vault_review_browser_plugins_ui` (E4) | 6 | 6 | 2 | 2 | `SM2-24` |
| **F19** 4-Tier Capability-Gated Plugin System V2, MCP Sync & Transactional Skills | `f19` | `area_vault_review_browser_plugins_ui` (E4) | 6 | 6 | 3 | 1 | `SM1-12` |
| **F20** Unified RPC v3, E2EE v2 HKDF-SHA256, Ephemeral VMs, STT, Artifacts & Watchdog | `f20` | `area_connectors_relay_groups` (E2) | 6 | 6 | 2 | 4 | `SM1-11` |
| **F21** Server REST/WS/Webhook API, Telegram IM Fusion, Client & `fabushi` CLI | `f21` | `area_connectors_relay_groups` (E2) | 6 | 6 | 1 | 9 | *(401 auth, CLI exit 3, zero-leak)* |
| **F22** Native GPUI Desktop UI Surfaces & `bot-browser` Design Mode | `f22` | `area_vault_review_browser_plugins_ui` (E4) | 6 | 6 | 1 | 3 | *(headless contracts + `SecuritySurface`)* |
| **F23** Opaque-Box E2E Suite Self-Integrity & Threshold Enforcement | `f23` | `area_vault_review_browser_plugins_ui` (E4) | 6 | 6 | 1 | 1 | *(`12/12` live meta-tests PASS)* |
| **F24** Ledger Preservation, CI/Deploy Workflows, Version `1.7.0` & Build Hygiene | `f24` | `area_connectors_relay_groups` (E2) | 6 | 6 | 1 | 1 | *(3-file `1.7.0`, 6-gate ledger regression)* |

---

## 4. Holistic Track-Level Verification Gate Proof (5/5 UNANIMOUS PASS)

| Verifier | Artifact | Verdict | Key Independently Verified Proof |
|---|---|---|---|
| **Track Forensic Auditor R2** | `.agents/teamwork/orca_e2e_track_auditor_r2/handoff.md` | **AUDIT VERDICT: CLEAN** | `353` collected on pytest `9.2.0` & `7.4.4` (`-W error`); `144/144/28/14` + `23` self; `24/24` features; `40/40` edge cases; `0` skips/xfails/mocks/3rd-party imports/vacuous asserts; `0` duplicates (`>=0.70` Jaccard across `62,128` pairs); `23/23` self-checks & `12/12` F23 PASS; `0` caches/leaks |
| **Track Primary Reviewer 1 R2** | `.agents/teamwork/orca_e2e_track_rev1_r2/handoff.md` | **VERDICT: APPROVE** | Dual-runner `353/353` (`0` warnings), `run_e2e.py` exact tier counts, `23/23` self-checks (`ResourceWarning=error`), `12/12` F23 PASS, `40` files, `100%` `TEST_INFRA.md` alignment |
| **Track Secondary Reviewer 2 R2** | `.agents/teamwork/orca_e2e_track_rev2_r2/handoff.md` | **VERDICT: APPROVE** | `0` cross-area RPC/REST key case-collisions across all `214` RPCs & `79` REST schemas (`E5 ⊆ E1–E4`), `0` 2-arg `.get()` defaults in E4, `35/35` self + T1/T2 F23 PASS on fallback pytest `7.4.4` |
| **Track Adversarial Challenger 1 R2** | `.agents/teamwork/orca_e2e_track_chal1_r2/handoff.md` | **VERDICT: CONFIRM** | All `309/309` product tests fail closed (`0` false passes, `0` skips, `0` errors) across ALL 5 whole-suite hostile dummy SUTs (`404`, `200 ok=true {}`+exit-0 CLI, `200 invalid_argument`, `400 invalid_argument`+exit-1 CLI, hybrid REST-400/RPC-200) + 11 targeted stubs; `0` `ResourceWarning`/temp/cache leaks |
| **Track Adversarial Challenger 2 R2** | `.agents/teamwork/orca_e2e_track_chal2_r2/handoff.md` | **VERDICT: CONFIRM** | Full AST anti-cheating audit clean across all `40` files; `6/6` adversarial dummy SUTs `0` fake passes; `32/32` `run_e2e.py` CLI/CWD probes + `4/4` live `--tier self` runs pass from both repo root & external CWD with `0` `__pycache__`/`.pytest_cache`/`*.pyc`/`/tmp` leaks |
