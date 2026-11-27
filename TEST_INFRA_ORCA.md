# Test Infrastructure — Orca Pure-Rust Absorption (`gpui-telegram` / Fabushi v1.7.0)

- **Suite Location**: `tests/orca_e2e/`
- **Authoritative Requirements**:
  - `.agents/teamwork/ORIGINAL_REQUEST.md`
  - `PROJECT.md` (Features `F01`–`F24`, Interface Contracts §§1–5)
  - `.agents/teamwork/orca_survey_spec_miner_1/orca_core_spec.md` (32 features, 14 edge cases `SM1-01`..`SM1-14`)
  - `.agents/teamwork/orca_survey_spec_miner_2/orca_bots_and_designs_spec.md` (23 groups, 26 edge cases `SM2-01`..`SM2-26`)
  - `.agents/teamwork/orca_survey_explorer_3/gpui_telegram_survey.md`
- **Isolation Guarantee**: Pure-Python opaque-box E2E suite; **zero** edits to `Cargo.toml`, `Cargo.lock`, `crates/**`, or `.github/**`.

---

## 1. Test Framework & Runtime Environment

- **Runner**: `pytest` via `/opt/pipx/bin/pytest` (`pytest 9.2.0` on Python `3.12.3`), with automatic fallback to `/usr/bin/python3 -m pytest` (`pytest 7.4.4`).
  - **CRITICAL PATH GOTCHA**: Default `python3` resolves to pyenv (`/opt/pyenv/shims/python3`, Python `3.12.16`), which does **NOT** have `pytest` installed. Always run via `tests/orca_e2e/run_e2e.py` or `/opt/pipx/bin/pytest`.
- **Zero Third-Party Dependencies**: Because `/opt/pipx/venvs/pytest` is isolated (`include-system-site-packages = false`, zero pytest plugins), the entire harness and test suite uses **Python 3.12 stdlib + `pytest` core ONLY** (`http.client`, `http.server`, `socket`, `struct`, `hashlib`, `hmac`, `base64`, `json`, `sqlite3`, `subprocess`, `tempfile`, `pathlib`, `threading`, `re`, `shutil`, `time`, `uuid`).
- **Working-Tree Cleanliness**:
  - `tests/orca_e2e/pytest.ini` configures `addopts = -p no:cacheprovider --strict-markers` (prevents untracked `.pytest_cache/` creation).
  - `tests/orca_e2e/run_e2e.py` sets `PYTHONDONTWRITEBYTECODE=1` (prevents untracked `__pycache__/` creation).
  - All temporary SQLite databases, bot home directories, Git worktree fixtures, media CAS stores, and CLI sessions use `tempfile.TemporaryDirectory` with RAII teardown cleanup (`AGENTS.md §1.3`).
- **Canonical Commands**:
  ```bash
  # Discover and verify collection across all Tiers 1-4 (no SUT required)
  /opt/pipx/bin/pytest -c tests/orca_e2e/pytest.ini --collect-only tests/orca_e2e

  # Run harness self-integrity checks (proves harness catches failures & never vacuously passes)
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier self

  # Run specific E2E tiers against built binaries (target/debug/gpui_telegram_server & target/debug/fabushi)
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 1
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 2
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 3
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 4
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier all

  # Point at an already-running server instance (optional)
  /usr/bin/python3 tests/orca_e2e/run_e2e.py --tier 1 --server-url http://127.0.0.1:18080
  ```

---

## 2. External Interfaces Exercised (Opaque-Box)

Tests interact strictly through documented user/system boundaries (`PROJECT.md § Interface Contracts`):

| Interface | Mechanism | Launch / Connection Contract |
|---|---|---|
| **Server HTTP / REST** | `gpui_telegram_server` subprocess + `http.client` | Built at `target/debug/gpui_telegram_server`. Takes **no CLI args**; configured via env: explicit free `PORT=<ephemeral>`, `DATABASE_URL=<tmp>/e2e.db`, `MEDIA_STORAGE_PATH=<tmp>/media`, `GPUI_OFFLINE_TEST=1` (never pass `PORT=0` because server binds `0.0.0.0:<PORT>` without printing bound port). Health poll `GET /health` -> `200 OK`. Auth bootstrap via `POST /api/v1/auth/register` -> `201 Created` Bearer token. Exercises `/api/v1/orca/*` & `/api/v1/orca/webhooks/{hook_id}/ingest`. |
| **Unified RPC** | `POST /api/v1/orca/rpc` | Envelope `{"method": "<ns.op>", "params": {...}, "protocol_version": 3, "caller_scope": "...", "operation_id": "..."}` -> `{"ok": true, "result": ...}` or `{"ok": false, "error": {"code": "...", "message": "..."}}`. |
| **Wake WebSocket** | Pure-stdlib RFC 6455 `WsClient` | Connects to `ws://127.0.0.1:<port>/api/v1/orca/wake?token=<token>` (or Bearer header). Asserts content-free wake frames, close `4409` on superseding socket, close `1009` on oversize inbound frame. |
| **Loopback Tool Relay** | Raw `http.client.HTTPConnection` | Queries relay address/grant from server (`127.0.0.1:<ephemeral>`). Sends raw `Origin`, spoofed `Host`, disallowed methods (`PUT`/`OPTIONS` -> `405`), invalid Bearer (`401`), unauthorized scope (`404`), burst concurrency (`429`), upstream `401` single OAuth refresh retry, mid-stream revoke (`404` / stream abort). |
| **`fabushi` CLI** | `subprocess.run` on `target/debug/fabushi` | Ambient `FABUSHI_*`/`GPUI_TELEGRAM_*`/`GPUI_TG_*` stripped; isolated `FABUSHI_CLI_SESSION_PATH=<tmp>/cli_session.json` & `FABUSHI_HOME=<tmp>/home`; invokes `--server-url <url> --token <tok> --remote <subcmd> ... --json`. Exit `0` JSON stdout on success; non-zero with `{"error": {"code": "...", "message": "..."}}` on failure; exit `3` on plugin `approval_required`. |
| **Filesystem / Git / SQLite Fixtures** | `tempfile.TemporaryDirectory`, `/usr/bin/git`, stdlib `sqlite3` | Real bot home folders (`SOUL.md`, `MEMORY.md`, `AGENTS.md`, `CLAUDE.md`, `bot.json`, `memory/journal/YYYY-MM.md`, `memory/archive.md`), real Git repos with `.worktreeinclude` & `orca.yaml`, real SQLite DBs testing `PRAGMA user_version` forward-compat (`items_read_only`), `AFTER UPDATE` triggers, and `SQLITE_CORRUPT` cache auto-rebuild. |
| **Desktop UI (`F22`) & Release/Ledger (`F24`)** | Backing API/CLI state contracts + targeted headless desktop contract tests + repo/script inspection | Verifies `F22` view-model/API backing transitions & headless desktop view contracts (`NavDestination::OrcaBots`, `ActiveWorkspacePanel::{OrcaBots, OrcaIde}`, `DesignModeCapture`, `SecuritySurface`), plus `F24` 3-file version `1.7.0` pins, `Cargo.toml` lean profiles/members, `ci.yml`/`deploy-backend.yml`/`release.yml`, `clean_build_garbage.py`, and `verify_ledger_closure.py`. |

---

## 3. Harness Module Contracts (`tests/orca_e2e/harness/`)

All area test modules (`E1`–`E5`) MUST import exclusively from `tests.orca_e2e.harness` (or `harness` via `pythonpath = tests/orca_e2e`):

1. **`harness/config.py`**:
   - `REPO_ROOT: Path`, `SERVER_BIN: Path`, `CLI_BIN: Path`, `PYTEST_BIN: str`
   - Canonical spec constants matching `PROJECT.md §1`: `BOT_AGENTS` (22 tuples), `BOT_ERROR_CODES`, `IDE_ERROR_CODES`, `BOT_MEMORY_INDEX_LINE_CAP = 200`, `READD_GUARD_DAYS = 14`, `TIDY_UP_TARGET_FACTS = 60`, `TIDY_UP_MAX_NEW_FACTS = 6`, `TIDY_UP_MAX_FILED = 20`, `BOT_MAX_MEMBERS = 200`, `BOT_MAX_CONNECTOR_WORKSPACES = 8`, `WEBHOOK_HOOK_HOURLY_CAP = 200`, `WEBHOOK_ACCOUNT_HOURLY_CAP = 300`, `ORCA_YAML_MAX_BYTES = 262_144`, `WORKTREE_INCLUDE_MAX_BYTES = 2_147_483_648`, `WORKTREE_INCLUDE_MAX_ENTRIES = 50_000`, `CLOSED_TAB_TOMBSTONE_DAYS = 30`, `RUNTIME_PROTOCOL_VERSION = 3`, `E2EE_V2_KEY_SCHEDULE_BYTES = 96`, `TEMPLATE_IDS_24`, `BUNDLED_SKILLS_13`, `TRIGGER_SOURCES_6_KINDS_31`, `WEBHOOK_ADAPTERS_6`, `TAB_CONTENT_TYPES_10`.
2. **`harness/transport.py`** (HTTP/REST/RPC + Pure-Stdlib RFC 6455 WebSocket):
   - `HttpResponse(status: int, reason: str, headers: dict[str, str], body: bytes)` with `.text`, `.json()`, `.assert_status(*codes)`, `.assert_error_code(code)`.
   - `RawHttpClient(host: str, port: int, timeout: float = 15.0)`: `.request(method, path, headers=None, body=None, json_body=None)` preserving raw `Host`/`Origin`/method.
   - `OrcaApiClient(base_url: str, token: str | None = None)`: `.get()`, `.post()`, `.patch()`, `.delete()`, `.rpc(method, params=None, protocol_version=3, caller_scope="desktop", operation_id=None)` with fail-closed assertions (`assert_ok()`, `assert_rpc_error(code)`).
   - `WsFrame(opcode: int, payload: bytes, close_code: int | None = None, close_reason: str = "")`
   - `WsClient.connect(url: str, headers: dict[str, str] | None = None, timeout: float = 10.0)` context manager with `.send_text()`, `.send_binary()`, `.send_raw_oversize()`, `.recv(timeout=5.0)`, `.recv_until_close(timeout=5.0)`, `.close()`.
3. **`harness/runners.py`** (Server Lifecycle + Isolated CLI Runner):
   - `allocate_free_port() -> int` (loopback `127.0.0.1:0` ephemeral probe).
   - `AuthSession(user_id: int, username: str, token: str, session_id: str)`.
   - `OrcaServerHandle`: manages subprocess lifecycle (`start()`, `stop()`, `restart()`, `base_url`, `ws_url`, `data_dir`, `db_path`, `register_user(...) -> AuthSession`, `api_client(session) -> OrcaApiClient`, `tail_logs()`). Raises `AssertionError` immediately (NEVER skips!) if server binary is missing or fails to boot.
   - `CliResult(returncode: int, stdout: str, stderr: str, cmd: list[str])` with `.json()` (parses stdout or stderr JSON), `.assert_ok() -> Any`, `.assert_error(expected_code: str, expected_exit: int | None = None)`.
   - `OrcaCliRunner(cli_bin: Path, server_url: str, token: str, work_dir: Path)`: `.run(*args, json_mode=True, stdin_text=None, extra_env=None, timeout=20.0) -> CliResult`. Fails cleanly if binary is absent (NEVER skips!).
4. **`harness/mock_servers.py`**:
   - Strictly for **external third-party upstreams** (never mocking product code!):
     - `MockMcpUpstreamServer`: loopback `ThreadingHTTPServer` recording received headers/bodies, supporting programmable `fail_401_once` (for OAuth refresh retry), `streaming_sse` (for mid-stream revoke abort detection), and custom JSON-RPC replies.
     - `MockOAuthServer`: loopback OAuth 2.1 + PKCE + DCR (`/register`, `/token`, `/revoke`, `/.well-known/oauth-authorization-server`) recording refresh/revoke calls.
     - `MockWebhookSink`: loopback receiver capturing outbound webhook/Slack mirror posts with timestamps for ordering-barrier assertions.
5. **`harness/fixtures.py`**:
   - `TempWorkspace`: RAII `tempfile.TemporaryDirectory` wrapper.
   - `GitRepoFixture`: initializes real local `git` repo (`user.name`/`user.email`), creates commits, `.gitignore`, `.worktreeinclude`, `orca.yaml`, untracked/ignored files, and branches.
   - `BotHomeFixture`: builds realistic bot homes (`SOUL.md`, `MEMORY.md`, `AGENTS.md`, `CLAUDE.md`, `bot.json`, `memory/`, `skills/`) and foreign OpenClaw/Hermes/OpenBot import directories.
   - `SqliteFixtureHelper`: opens WAL SQLite DBs, mutates `PRAGMA user_version`, runs simulated legacy `UPDATE` statements to verify `AFTER UPDATE` triggers, and injects torn pages (`SQLITE_CORRUPT`).

---

## 4. Shared Pytest Fixtures (`tests/orca_e2e/conftest.py`)

- **CLI Options**: `--server-url` (optional external server URL; overrides auto-spawn), `--server-bin` (defaults to `target/debug/gpui_telegram_server`), `--cli-bin` (defaults to `target/debug/fabushi`).
- **Fixtures**:
  - `orca_server` (module-scoped `OrcaServerHandle` with isolated temp dir & ephemeral port, auto-stopped on teardown)
  - `fresh_orca_server` (function-scoped `OrcaServerHandle` for restart/corruption/stateful isolation tests)
  - `auth_session` (function-scoped newly registered user via `POST /api/v1/auth/register` -> `AuthSession`)
  - `orca_api` (function-scoped authenticated `OrcaApiClient`)
  - `orca_cli` (function-scoped isolated `OrcaCliRunner` bound to `orca_server` & `auth_session`)
  - `temp_workspace` (function-scoped `TempWorkspace` RAII directory)
  - `git_repo` (function-scoped initialized `GitRepoFixture`)
  - `bot_home_factory` (factory creating isolated `BotHomeFixture` dirs inside `temp_workspace`)
  - `mock_mcp_upstream` / `mock_oauth_server` / `mock_webhook_sink` (function-scoped loopback upstream mocks auto-shutting down on teardown)

---

## 5. Tier Taxonomy, Thresholds & Requirement-Area Decomposition

For **N = 24** features (`F01`–`F24`), this suite targets **330 concrete opaque-box tests** (exceeding the minimum **276**):

| Tier | Marker | Minimum Required | Target Count | Scope |
|---|---|---:|---:|---|
| **Tier 1 — Feature Coverage** | `@pytest.mark.tier1` | >= 5/feature (>= 120) | **144** (6/feature) | Isolated happy-path behavioral verification per feature `F01`–`F24` |
| **Tier 2 — Boundary & Corner Cases** | `@pytest.mark.tier2` | >= 5/feature (>= 120) | **144** (6/feature) | Negative inputs, error codes, security gates, concurrency/CAS races, and 100% of `SM1-01..14` + `SM2-01..26` |
| **Tier 3 — Cross-Feature Pairwise** | `@pytest.mark.tier3` | >= 24 | **28** | Pairwise cross-feature interaction matrices across Bots, Orchestration, Worktrees, Relay/Connectors, Vault/Review, Plugins, UI & Release |
| **Tier 4 — Real-World Scenarios** | `@pytest.mark.tier4` | >= 12 | **14** | Multi-step realistic end-to-end user workloads spanning creation, execution, failure recovery, security enforcement & persistence across restarts |
| **Harness Self-Checks** | `@pytest.mark.harness_self_check` | *(meta, excluded from T1–T4)* | **12+** | Proves harness fixtures, WS close-code parser, raw HTTP client, mock servers & fail-closed missing-endpoint diagnostics catch real failures |

### Feature-Area Subdirectories (Disjoint Ownership)

| Area | Directory | Features | Tier 1 | Tier 2 | Mapped Upstream Edge Cases |
|---|---|---|---:|---:|---|
| **E1** | `tests/orca_e2e/area_bots_memory_items/` | F01, F02, F03, F04, F05, F07 | 36 | 36 | `SM2-01..04(bot/items)`, `SM2-05..11`, `SM2-16`, `SM2-19`, `SM2-20`, `SM2-25` |
| **E2** | `tests/orca_e2e/area_connectors_relay_groups/` | F06, F08, F09, F20, F21, F24 | 36 | 36 | `SM1-11`, `SM2-04(groups)`, `SM2-12..15`, `SM2-17`, `SM2-18`, `SM2-21`, `SM2-26` |
| **E3** | `tests/orca_e2e/area_orchestration_worktrees_pty/` | F10, F11, F12, F13, F14, F15 | 36 | 36 | `SM1-01`, `SM1-02`, `SM1-06..10`, `SM1-14`, `SM2-22`, `SM2-23` |
| **E4** | `tests/orca_e2e/area_vault_review_browser_plugins_ui/` | F16, F17, F18, F19, F22, F23 | 36 | 36 | `SM1-03..05`, `SM1-12`, `SM1-13`, `SM2-24` |
| **E5** | `tests/orca_e2e/tier3_pairwise/` & `tier4_scenarios/` | All `F01`–`F24` | — | — | **28** Tier 3 + **14** Tier 4 end-to-end tests |

---

## 6. Non-Vacuous Assertion & Anti-Cheating Guarantees

1. **Fail-Closed on Missing Implementation**: If `gpui_telegram_server` or `fabushi` is not built, or an `/api/v1/orca/*` route returns `404`/`501`, or a CLI command exits non-zero as unknown, every dependent test **MUST FAIL** with explicit status/stdout/stderr diagnostics — **NEVER** `pytest.skip`, **NEVER** `xfail`, **NEVER** pass vacuously.
2. **Zero Product Mocking**: `mock_servers.py` simulates **only** external third-party upstreams (remote MCP HTTP servers, external OAuth providers, outbound webhook receivers). All Orca routes, RPCs, CLI commands, Tool Relay security gates, SQLite schemas, and parsers are tested directly against the compiled product.
3. **Traceable Markers**: Every Tier 1–4 test declares its tier (`@pytest.mark.tier1..4`), its feature(s) (`@pytest.mark.f01..24`), and (for upstream edge cases) `@pytest.mark.edge_case("SM1-XX" | "SM2-XX")`.
4. **Hygiene & Teardown**: Every test cleans up child processes, sockets, and temp directories via RAII (`try/finally` + `TemporaryDirectory`), leaving `git status` clean and zero leaked processes. Any test invoking `cargo` runs `python3 scripts/clean_build_garbage.py` immediately afterward (`AGENTS.md`).
