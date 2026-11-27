# E2E Test Infrastructure & Specification — Apple-Style Sidebar & Creative Content Ecosystem

- **Feature Track**: Apple-Style Primary Sidebar (Books, Music, Games, App Store, My Creations), Bot-Assisted Cloud Creation, Review/Publishing FSM & Public Bot Recommendations
- **Authoritative Request**: `.agents/teamwork/ORIGINAL_REQUEST.md` (`## Follow-up — 2026-11-24T08:43:16Z`)
- **Architecture & Public Contracts**: `/home/antigravity/bhrumom/gpui-telegram/PROJECT.md`
- **Build Hygiene Mandate**: `/home/antigravity/bhrumom/gpui-telegram/AGENTS.md`
- **Track Status**: **Phase A PASSED (`5/5 APPROVE / CLEAN`)** — 94/94 opaque-box HTTP/WS/Bot/Persistence tests compiled cleanly via `--no-run` (0 errors, 0 warnings) and verified against M1–M6 contracts; ready for Milestone M7 runtime execution. Phase B Headless GPUI UI binary (`apple_sidebar_ui_e2e.rs`) & `TEST_READY.md` certification follow Milestone M6.

---

## 1. Executive Summary & Opaque-Box Testing Philosophy

This document defines the independent, requirement-driven **4-Tier Opaque-Box E2E Test Infrastructure** for the Apple-Style Sidebar & Creative Content Ecosystem in `gpui-telegram`.

All Phase A tests are derived exclusively from the user requirements in `ORIGINAL_REQUEST.md` (`2026-11-24T08:43:16Z`) and the public interface contracts in `PROJECT.md § Feature Inventory` + `§ Interface Contracts`:

> 我现在需要给应用增加一个侧边栏，按照苹果的风格设计，增加图书，音乐，游戏，appstore入口，每个板块的界面按照苹果的图书app，apple musicapp，游戏app，appstore来设计，用户可以使用bot创作图书音乐游戏，这些内容会保存在用户的云端数据，在侧边栏旁边进去也可以找到用户创作的内容，创作完成后可以推送审核上架，上架就可以在这些板块里面被找到，并且公开上架的内容，bot是可以调用推荐的

### Core Principles
1. **100% Opaque-Box Behavioral Verification**:
   - Phase A test binaries (`tests/e2e_suite/tests/apple_sidebar_tier{1..4}_*.rs`) drive a real file-backed `EphemeralTestServer` over live TCP HTTP REST (`reqwest`), WebSocket (`tokio_tungstenite`), and JSON payloads (`serde_json`) — ZERO imports of unbuilt internal Rust structs from M1–M6.
   - Progressive Testability: Every Phase A binary compiles cleanly TODAY (`cargo test -p e2e_suite --test apple_sidebar_tier1_feature ... --no-run`) without touching `crates/`, and executes end-to-end once M1–M6 land in Milestone M7.
2. **Zero-Placeholder Integrity**:
   - Every test function asserts concrete HTTP status codes, JSON contract fields, state transitions, persistence across `EphemeralTestServer::restart()`, and strict visibility/security boundaries.
   - Zero `assert!(true)`, zero `#[ignore]`, zero early `return;` skips on `404`/`501`, and zero loose `status.is_success()` checks without body assertions.
3. **4-Layer Visibility Isolation Verification**:
   - Adversarial Tier 2 and Tier 4 tests verify across EVERY public surface (`/api/v1/catalog`, `/catalog/{books,music,games,appstore}`, `/catalog/recommend`, `/api/v1/bots/inline-query`, `@CreatorStudioBot /recommend`) in both **Chinese (CJK)** and **English** that `Draft`, `PendingReview`, `Rejected`, `Unlisted`, `Unpublished`, and soft-deleted items **never leak** to non-owners or bots.
4. **Build & Runtime Hygiene (`AGENTS.md`)**:
   - Root `Cargo.toml` profiles keep `debug = 0`, `strip = "debuginfo"`, `incremental = false`; single workspace `target/` directory (never pass `--target-dir`).
   - Run `python3 scripts/clean_build_garbage.py` immediately after EVERY `cargo` build/check/test invocation.
   - All temporary SQLite WAL, Tantivy, and CAS directories are RAII-managed via `EphemeralTestServer` (`TempTestDir`) / `tempfile::TempDir`, and every async test explicitly calls `server.stop().await`.

---

## 2. 4-Tier Verification Architecture & Test Matrix

```text
                  /\
                 /  \   Tier 4: Real-World Multi-User Scenarios & Cold Restart Durability (7 tests)
                /----\  tests/e2e_suite/tests/apple_sidebar_tier4_scenarios.rs
               /      \
              /--------\ Tier 3: Pairwise Cross-Feature Combinations (14 tests)
             /          \ tests/e2e_suite/tests/apple_sidebar_tier3_cross_feature.rs
            /------------\
           /              \ Tier 2: Boundary, Corner & Adversarial Zero-Leak Security (33 tests)
          /----------------\ tests/e2e_suite/tests/apple_sidebar_tier2_boundary.rs
         /                  \
        /--------------------\ Tier 1: Isolated Feature Coverage >=5/area (40 tests)
       /______________________\ tests/e2e_suite/tests/apple_sidebar_tier1_feature.rs
       + Phase B (Post-M6): Headless GPUI UI E2E (tests/e2e_suite/tests/apple_sidebar_ui_e2e.rs)
```

| Tier | Binary Target (`-p e2e_suite --test ...`) | Count | Primary Verification Focus |
|---|---|---|---|
| Shared Helper | `tests/e2e_suite/tests/apple_sidebar_common/mod.rs` | — | Opaque-box `EphemeralTestServer` lifecycle, auth/admin elevation, Book/Music/Game/App JSON fixtures, Bot DM polling, CAS raw media upload/download, MiniApp publish, raw JSON WS event waiter |
| **Tier 1** | `apple_sidebar_tier1_feature` | 40 | Isolated happy-path coverage (>= 5 per area): Cloud Creations CRUD (6), Delta Sync (5), Review/Publish FSM (6), Vertical Catalogs & Search/Sort (6), Per-User States/Ratings/Scores (5), `@CreatorStudioBot` Creation Commands (6), Public Recommendations & Inline Queries (6) |
| **Tier 2** | `apple_sidebar_tier2_boundary` | 33 | Validation `422` errors (6), `409 revision_conflict` CAS & `401`/`403`/`404` isolation (6), invalid FSM transitions (5), strict EN/CJK zero-leak across Draft/Pending/Rejected/Unlisted/Unpublished/Deleted (6), CJK/FTS/wildcard/bot boundaries (5), rating/review/score bounds (5) |
| **Tier 3** | `apple_sidebar_tier3_cross_feature` | 14 | Pairwise interactions: Bot Create -> Cloud -> Edit -> Submit -> Approve -> Vertical Catalog -> Bot Recommend -> `tg://apple/*` deep-link; live-edit frozen published snapshot; App Store merge (App + MiniApp + featured verticals); CAS public asset allow/revoke; WS `UserCreationChanged` & `PublicCatalogChanged`; rankings; inline query & delta sync |
| **Tier 4** | `apple_sidebar_tier4_scenarios` | 7 | Multi-user journeys across Author/Composer/GameDev/Reviewer/Readers/Players with `EphemeralTestServer::restart()` cold persistence durability, multi-account cloud isolation, bilingual EN/ZH discovery, and mixed-visibility zero-leak |
| **Phase B UI** | `apple_sidebar_ui_e2e` *(blueprint in §6, built after M6)* | 16+ | Headless GPUI `Application::headless()` + `AppShell::inspect_state` verifying `AppleAppSidebar`, `ContentHubWorkspace`, Books/Music/Games/AppStore/MyCreations, `PublishReviewModal`, `IconName::ALL == 64` + `EXTRA`, `tg://apple/*` deep-links |

---

## 3. Test Harness, Runtime Conventions & Exact Runner Commands

### 3.1 Ephemeral Test Server & Opaque-Box Driver Mechanics
- **Real File-Backed Server**: Every test boots `gpui_telegram_test_harness::tier4_e2e::EphemeralTestServer::start().await?`, which starts a genuine Axum + SQLite WAL (`e2e_server.db`) + SHA-256 `CasStore` + `TantivyIndexer` + `GatewayHub` + `FabushiBotHost` server bound to `127.0.0.1:0` inside an RAII `TempTestDir`.
- **Cold Restart Durability**: `EphemeralTestServer::restart()` shuts down the live server and re-boots on the same `TempTestDir` & SQLite WAL file on a **new** ephemeral port; `apple_sidebar_common::restart_server` refreshes `E2eSystemDriver::new(server.base_url())` immediately after restart.
- **Auth & Admin Reviewer Elevation**:
  - Standard actors register via `POST /api/v1/auth/register` (`201 Created`, passwords `>= 8` chars) and re-login via `POST /api/v1/auth/login` (`200 OK`).
  - Because `auth_register` always inserts `is_support = 0` and no self-service admin route exists, `apple_sidebar_common::promote_user_to_admin` follows the repository's canonical pattern (`hermes_gateway_and_bot_bugfix_tests.rs:647-655`): `UPDATE users SET is_support = 1 WHERE user_id = ?1` via `server.state().unwrap().db.pool().with_writer(...)` (persists across `restart()`).
- **Zero Unbuilt Internal Imports**:
  - `apple_sidebar_common/mod.rs` uses ONLY `reqwest`, `serde_json`, `tokio_tungstenite`, `futures`, `uuid`, `gpui_telegram_core::error::TelegramError`, and `gpui_telegram_test_harness::tier4_e2e::{E2eSystemDriver, EphemeralTestServer}`.
  - WebSocket frames are parsed directly from raw UTF-8 JSON (`serde_json::Value`), matching BOTH `{"type":"Update","payload":{"type":"<Variant>","data":{...}}}` and `{"type":"PtsPush","payload":{"event":{"type":"<Variant>","data":{...}}}}` while ignoring unrelated presence frames (`UserStatusChanged`, `BotActivityChanged`).
- **CAS Media Fixtures**:
  - Unique per-test PNG and RIFF/WAV byte fixtures are uploaded via raw-bytes `POST /api/v1/media/upload?filename=<name>&mime_type=<mime>&is_voice=false` (with NO `chat_id` query param so non-owners receive `403 Forbidden` before publish, `200 OK` after publish, and `403 Forbidden` after unpublish). Every fixture embeds unique bytes to avoid SHA-256 `INSERT OR REPLACE` deduplication collisions across users.
- **Bot DM & Reply Polling**:
  - DM chats with `@CreatorStudioBot` (`UID 777_020`) are opened via `POST /api/v1/chats` (`{"peer_user_id": 777020}`) or `POST /api/v1/contacts/add` (`{"username": "CreatorStudioBot", "is_bot": true}`).
  - Because bot replies run asynchronously via `tokio::spawn`, helpers poll `GET /api/v1/chats/{chat_id}/messages?limit=100` for `m["id"] > sent_id && m["sender_id"] == 777020`, inspecting `m["text"]`, `m["content"]` (`{"type":"GenerativeCard","data":{...}}`), and `m["inline_keyboard"]` (never `reply_markup`).

### 3.2 Canonical Commands & Pass/Fail Semantics

#### Phase A — Compile-Clean Verification (Available Immediately Before M1–M6 Complete)
```bash
cargo test -p e2e_suite \
  --test apple_sidebar_tier1_feature \
  --test apple_sidebar_tier2_boundary \
  --test apple_sidebar_tier3_cross_feature \
  --test apple_sidebar_tier4_scenarios \
  --no-run && python3 scripts/clean_build_garbage.py
```
- **Pass criteria (Phase A)**: Exit code `0`, zero compiler errors, zero regressions to existing `e2e_suite` test targets, and clean post-build `target/` garbage sweep.

#### Milestone M7 — Full Live Runtime Execution (Sequential Across Tiers 1–4)
```bash
cargo test -p e2e_suite --test apple_sidebar_tier1_feature -- --nocapture && \
cargo test -p e2e_suite --test apple_sidebar_tier2_boundary -- --nocapture && \
cargo test -p e2e_suite --test apple_sidebar_tier3_cross_feature -- --nocapture && \
cargo test -p e2e_suite --test apple_sidebar_tier4_scenarios -- --nocapture && \
python3 scripts/clean_build_garbage.py
```
- **Pass criteria (M7)**: Exit code `0`, `100%` of tests passing (`0 failed`, `0 ignored`), zero leftover temp files in `/tmp` or workspace.

---

## 4. Public Interface Contracts Verified by Phase A Suites

### 4.1 Wire Schema & Endpoints (`PROJECT.md § Interface Contracts`)
- **Enums (`snake_case`)**:
  - `kind`: `"book" | "music" | "game" | "app"`
  - `review_status`: `"draft" | "pending_review" | "published" | "rejected" | "unpublished"`
  - `visibility`: `"private" | "unlisted" | "public"`
  - `sort`: `"featured" | "popular" | "top_rated" | "newest"`
  - `decision`: `"approve" | "reject"`
- **User Cloud Creations (`/api/v1/creations/*`, Bearer auth, strict `owner_user_id` isolation)**:
  - `POST /api/v1/creations` (`201`/`200` `UserCreationDto` with `creation_id`, `cloud_revision >= 1`, `review_status == "draft"`, `visibility == "private"`)
  - `GET /api/v1/creations/my?kind=&status=&q=` (`200` `{ items, latest_revision }`)
  - `GET /api/v1/creations/sync?since_revision=N` (`200` `{ latest_revision, items }` including soft-delete tombstones with `deleted_at_ms`)
  - `GET /api/v1/creations/{id}` (`200` owner draft; `200` frozen published snapshot for non-owner iff `published && (public|unlisted)`; else `403`/`404`)
  - `PUT /api/v1/creations/{id}` (`200` updated draft; `409` `revision_conflict` on stale `expected_revision`; live-edit preserves frozen live snapshot until re-approved)
  - `DELETE /api/v1/creations/{id}` (`200`/`204` soft-delete tombstone + immediate atomic purge from `public_catalog_entries` & `creation_public_assets`)
  - `POST /api/v1/creations/{id}/submit-review` (`200` `SubmitReviewResponse { creation: UserCreationDto, validation_errors: [] }` with `creation.review_status == "pending_review"`; `422` + non-empty `validation_errors` on invalid payload)
  - `POST /api/v1/creations/{id}/withdraw-review` (`200` `draft`)
  - `POST /api/v1/creations/{id}/unpublish` (`200` `unpublished` + immediate atomic removal from catalog & FTS5 & public CAS allowlist)
  - `GET /api/v1/creations/{id}/audit` (`200` immutable review transition history with `from_status`, `to_status`, `actor_user_id`, `created_at_ms`)
- **Authoritative Payload & Sub-Object Fields (aligned with M1 `crates/gpui_telegram_proto/src/creations.rs`, exercised 100% via opaque-box JSON)**:
  - `BookChapter`: requires `chapter_id`, `chapter_number: u32`, `title`, `pages`.
  - `MusicTrack`: `waveform_5bit: Vec<u8>` (send `[]`, never explicit `null`), `melody_notes`, `chord_progression`, `instruments`, `lyrics`, `audio_media_file_id`.
  - `TriviaQuestion`: requires `question_id`, `prompt`, `choices`, `correct_index`, `explanation`; `GameAchievementDef`: requires `achievement_id`, `title`, `description`, optional `target_score`, `icon_emoji`.
- **Moderation / Review Queue (`Bearer` + admin `is_support == true`)**:
  - `GET /api/v1/creations/review-queue?status=pending_review&kind=` (`200` `{ items }`)
  - `POST /api/v1/creations/{id}/review` (`200`, `{ decision, visibility?, reason?, notes? }`)
- **Public Catalog & Vertical Discovery (`Bearer`, physically isolated `public_catalog_entries` ONLY)**:
  - `GET /api/v1/catalog?kind=&q=&genre=&sort=&limit=&offset=`
  - `GET /api/v1/catalog/{books|music|games|appstore}` (`/appstore` returns `AppStoreCatalogResponse { featured, apps, mini_apps, cross_vertical }` aggregating published `App` creations, `mini_apps WHERE is_published = 1`, and featured published Books/Music/Games)
  - `GET /api/v1/catalog/recommend?q=&kind=&genre=&limit=` (`{ items }` with `deep_link: "tg://apple/<section>/<creation_id>"`)
  - `PUT /api/v1/catalog/{id}/state` (`UpsertCreationStateRequest { in_library, favorite, progress_percent, state_json }`) & `GET /api/v1/catalog/my-states` (`UserCreationStateDto { creation_id, in_library, favorite, progress_percent, state_json, updated_at_ms }`)
  - `POST /api/v1/catalog/{id}/reviews` (`CreateCreationReviewRequest { rating: 1..=5, title?, body }`) & `GET /api/v1/catalog/{id}/reviews`
  - `POST /api/v1/catalog/{id}/scores` (`{ score }`) & `GET /api/v1/catalog/{id}/leaderboard`
- **Bot Studio, Inline Queries & Realtime WS**:
  - `@CreatorStudioBot` (`UID 777_020`, `is_official = true`, `is_inline = true`) deterministic slash grammar:
    - `/start`, `/help`
    - `/newbook <title> | <summary>`
    - `/newmusic <title> | <summary>`
    - `/newgame <title> | <summary>`
    - `/mycreations`
    - `/editwork <creation_id> title=<new_title>`
    - `/submitwork <creation_id>`
    - `/recommend [<query>]` (supports English & CJK queries; empty query returns top featured public creations)
  - `GET /api/v1/bots/inline-query?bot_username=&query=&kind=&limit=` (published+public only; supports empty query browse)
  - `POST /bot/{token}/answerInlineQuery` (standard Telegram Bot API envelope `{"ok": true, "result": ...}`; rejects invalid token `401` and missing `inline_query_id` `400` with `ok: false`)
  - WS `/api/v1/ws?token=...`: perform `WsClientFrame::Ping`/`Pong` sync handshake on connect; handles `UpdateEvent::UserCreationChanged` (owner-only!) & `UpdateEvent::PublicCatalogChanged` (public-only, never emitted for `unlisted`)
  - Canonical deep-link URI: `tg://apple/<books|music|games|appstore|creations>[/<creation_id>][?tab=...&action=...]`

---

## 5. Phase A Test Catalog & Requirement Traceability

### 5.1 Tier 1 — Isolated Feature Coverage (`tests/e2e_suite/tests/apple_sidebar_tier1_feature.rs`, 40 tests)
- **Cloud Creations CRUD (F1, F4 — 6 tests)**:
  - `t1_01_create_and_fetch_book_cloud_draft` .. `t1_04_create_and_fetch_app_cloud_draft` (Book/Music/Game/App creation, default `draft` + `private`, monotonic `cloud_revision`, full payload round-trip)
  - `t1_05_list_my_creations_filters_by_kind_status_and_query`
  - `t1_06_update_and_soft_delete_cloud_creation_preserves_tombstone`
- **Cloud Delta Sync (F4 — 5 tests)**:
  - `t1_07_sync_from_zero_returns_all_creations_and_monotonic_revision`
  - `t1_08_sync_incremental_returns_only_newer_revisions`
  - `t1_09_sync_captures_update_incrementing_cloud_revision`
  - `t1_10_sync_delivers_soft_delete_tombstone_with_deleted_at_ms`
  - `t1_11_sync_at_latest_revision_returns_empty_delta`
- **Review & Publishing FSM (F5 — 6 tests)**:
  - `t1_12_submit_valid_creation_transitions_draft_to_pending_review`
  - `t1_13_withdraw_review_returns_pending_creation_to_draft`
  - `t1_14_admin_review_queue_lists_pending_submissions_by_kind`
  - `t1_15_admin_approve_public_publishes_creation_with_timestamps`
  - `t1_16_admin_reject_records_reason_notes_and_allows_resubmit`
  - `t1_17_owner_unpublish_and_audit_trail_records_all_transitions`
- **Vertical Catalogs & Search/Ranking (F6 — 6 tests)**:
  - `t1_18_catalog_books_returns_published_public_books`
  - `t1_19_catalog_music_returns_published_public_music`
  - `t1_20_catalog_games_returns_published_public_games`
  - `t1_21_catalog_appstore_returns_published_apps_and_featured_verticals`
  - `t1_22_catalog_search_filters_by_english_and_cjk_keywords_and_genre`
  - `t1_23_catalog_sorting_supports_featured_popular_top_rated_newest_and_pagination`
- **Per-User States, Ratings & Leaderboards (F6 — 5 tests)**:
  - `t1_24_user_book_reading_progress_and_bookmarks_cloud_state`
  - `t1_25_user_music_loved_playback_position_and_library_state`
  - `t1_26_user_game_save_state_and_app_install_state_in_my_states`
  - `t1_27_public_reviews_and_ratings_update_catalog_rating_avg_and_count`
  - `t1_28_published_game_scores_populate_sorted_leaderboard`
- **Bot `@CreatorStudioBot` Creation Commands (F7 — 6 tests)**:
  - `t1_29_creator_studio_bot_discoverable_and_responds_to_start_help`
  - `t1_30_bot_newbook_creates_cloud_book_draft_with_card_and_deep_link`
  - `t1_31_bot_newmusic_creates_cloud_music_draft_with_playable_tracks`
  - `t1_32_bot_newgame_creates_cloud_game_draft_with_playable_spec`
  - `t1_33_bot_mycreations_lists_users_cloud_creations`
  - `t1_34_bot_editwork_and_submitwork_manage_and_submit_draft`
- **Bot Public Recommendations & Inline Queries (F8 — 6 tests)**:
  - `t1_35_catalog_recommend_returns_ranked_public_items_with_apple_deep_links`
  - `t1_36_catalog_recommend_filters_by_kind_genre_and_limit`
  - `t1_37_bot_recommend_command_returns_published_items_in_english_and_chinese`
  - `t1_38_bot_recommend_reply_includes_canonical_tg_apple_deep_link_buttons`
  - `t1_39_inline_query_endpoint_returns_published_public_creations`
  - `t1_40_telegram_bot_api_answer_inline_query_accepts_results_payload`

### 5.2 Tier 2 — Boundary, Corner & Adversarial Zero-Leak Security (`tests/e2e_suite/tests/apple_sidebar_tier2_boundary.rs`, 33 tests)
- **Payload Validation (`422` + `validation_errors`) (6 tests)**: `t2_01` (empty book / <50 chars), `t2_02` (out-of-range BPM / empty tracks & notes), `t2_03` (unplayable game / invalid trivia index), `t2_04` (XSS `<script>` & `javascript:`), `t2_05` (oversized title / short summary / bad cover hex), `t2_06` (>4 MiB payload).
- **CAS Concurrency & Access Control (6 tests)**: `t2_07` (`409 revision_conflict`), `t2_08` (`401` unauthenticated), `t2_09` (`403`/`404` cross-user draft read/edit/submit/withdraw/unpublish/delete), `t2_10` (non-admin review forbidden), `t2_11` (audit log isolation), `t2_12` (`unlisted` direct fetch allowed, hidden from catalog).
- **Invalid Review FSM Transitions (5 tests)**: `t2_13` (`Draft -> Published` rejected), `t2_14` (reject without mandatory reason), `t2_15` (withdraw non-pending), `t2_16` (unpublish draft/rejected), `t2_17` (submit soft-deleted creation).
- **Adversarial Zero-Leak Public Catalog & Bot Isolation (6 tests)**: `t2_18` (`Draft`), `t2_19` (`PendingReview`), `t2_20` (`Rejected`), `t2_21` (`Unlisted`), `t2_22` (`Unpublished` immediate disappearance), `t2_23` (soft-deleted published item immediate purge) — each verified across `/catalog`, vertical endpoints, `/catalog/recommend`, `/api/v1/bots/inline-query`, and `@CreatorStudioBot /recommend` in BOTH English and Chinese!
- **CJK Search, Escaping, Pagination & Bot Security Boundaries (5 tests)**: `t2_24` (unspaced CJK substring/bigram search), `t2_25` (FTS5 syntax & SQL `LIKE` wildcard `%_\"*` safety), `t2_26` (limit/offset extremes), `t2_27` (bot cross-user `/editwork` & `/submitwork` blocked), `t2_28` (adversarial bot prompt targeting private draft ID cannot leak).
- **Ratings, Reviews & Scores Bounds (5 tests)**: `t2_29` (rating 0 / 6 rejected), `t2_30` (review on draft rejected), `t2_31` (score on non-game or draft game rejected), `t2_32` (duplicate user review upserts without inflating `rating_count`), `t2_33` (lower score preserves higher `best_score`).

### 5.3 Tier 3 — Pairwise Cross-Feature Combinations (`tests/e2e_suite/tests/apple_sidebar_tier3_cross_feature.rs`, 14 tests)
- `t3_01` / `t3_02` / `t3_03`: End-to-end Bot Create (Book / Music / Game) -> Cloud My Creations -> Edit -> Submit -> Approve -> Vertical Catalog -> Bot Recommend -> `tg://apple/<section>/<id>` deep-link check.
- `t3_04` / `t3_05`: Live-edit of a `Published` creation preserves frozen `published_snapshot_json` for non-owners until v2 is re-approved (and rejecting v2 keeps live integrity intact).
- `t3_06`: App Store (`/api/v1/catalog/appstore`) merges published `App` creations + published `mini_apps` (`POST /api/v1/bots` + `POST /api/v1/miniapps`) + featured published Books/Music/Games.
- `t3_07`: CAS media asset access (`GET /api/v1/media/{id}`) returns `403` to non-owner while `Draft`, `200` once `Published + Public`, and reverts to `403` immediately after `unpublish`.
- `t3_08` / `t3_09`: Realtime WS (`/api/v1/ws?token=...`) delivers `UserCreationChanged` to owner and `PublicCatalogChanged` (`published` / `removed`) on catalog transitions.
- `t3_10` / `t3_11`: Ratings & engagement update `top_rated` / `recommend` ranking; multi-player game scores & per-user states stay consistent.
- `t3_12` / `t3_13` / `t3_14`: Inline query live-reflects publish/unpublish; incremental delta sync reflects bot creations + review FSM changes; republish after unpublish restores public discovery & CAS media access.

### 5.4 Tier 4 — Real-World Multi-User Scenarios & Cold Restart Durability (`tests/e2e_suite/tests/apple_sidebar_tier4_scenarios.rs`, 7 tests)
- `t4_01`: Author writes Book via `@CreatorStudioBot` -> cloud edit -> submit -> admin approves -> Reader discovers in Apple Books, reads & saves progress/bookmarks, posts review -> `EphemeralTestServer::restart()` confirms cloud drafts, published catalog, reading progress, bookmarks, reviews, and audit history survive intact.
- `t4_02`: Composer creates Music with CAS WAV audio via Bot -> published -> Listener discovers in Apple Music, streams WAV, loves track & saves position -> Third user receives `@CreatorStudioBot /recommend` deep-link -> survives cold restart.
- `t4_03`: Game Creator builds playable game via Bot -> published -> 3 Players compete on leaderboard & save state -> descending leaderboard & scores survive cold restart.
- `t4_04`: Multi-vertical creator publishes Book + Music + Game + App + MiniApp -> verified across all 4 vertical endpoints, unified App Store, and bilingual EN/ZH Bot & inline recommendations.
- `t4_05`: Two independent accounts maintain separate cloud revisions, private drafts, and user states across re-login (`POST /api/v1/auth/login`) and server restart with zero cross-account bleed.
- `t4_06`: Full moderation journey: fail `422` validation -> fix -> submit -> reject -> revise -> approve v1 -> live-edit v2 while readers see v1 -> approve v2 -> unpublish -> verify zero-leak -> delete tombstone -> cold restart verifies tombstone & full audit trail.
- `t4_07`: Mixed-visibility matrix (`Draft`, `PendingReview`, `Rejected`, `Unlisted`, `Unpublished`, `Public`) across multiple creators and all 4 verticals verified before and after cold restart: zero leakage on every discovery & bot recommendation surface.

---

## 6. Phase B Blueprint — Headless GPUI UI E2E (`tests/e2e_suite/tests/apple_sidebar_ui_e2e.rs`)

> **Deferred to Phase B (Post-M6)**: `apple_sidebar_ui_e2e.rs` will be implemented and verified once Milestones M5–M6 land `AppleAppSidebar`, `ContentHubWorkspace`, the 4 Apple verticals, and `AppShell::handle_apple_deep_link`, followed by publishing `/home/antigravity/bhrumom/gpui-telegram/TEST_READY.md`.

### 6.1 Execution Architecture (No `#[gpui::test]` — Verified Real GPUI 0.2.2 Pattern)
- **Subprocess Main-Thread Trampoline**: Adopt the proven pattern from `crates/gpui_telegram_desktop/tests/m4_semantic_tree_rtc_tests.rs:43-141` (`std::env::current_exe()` re-exec with `CHILD_ENV`/`CHILD_TARGET_ENV`, RAII `TempTestDir` for `GPUI_TELEGRAM_CONFIG_DIR`, macOS `#[used] #[unsafe(link_section = "__DATA,__mod_init_func")]` main-thread hook, `catch_unwind` + `std::process::exit(0|101)`).
- **Live Server Wiring**: Parent `#[tokio::test]` boots `EphemeralTestServer::start()`, seeds creations/reviews/catalog items over HTTP, passes `server.base_url()` and auth tokens via env vars to the headless child, and calls `server.stop().await` on exit. **IMPORTANT**: Do **NOT** set `GPUI_OFFLINE_TEST=1` (used in `m4_semantic_tree_rtc_tests.rs:118`) for live-server headless UI tests (`ui_t3_03`, `ui_t4_01`, `ui_t4_02`), because `GPUI_OFFLINE_TEST=1` disables live HTTP/WS network sync!
- **Headless GPUI App Boot**: Inside child main thread, construct `Arc<TelegramClient>`, enter `let _rt = client.runtime().enter();`, boot `gpui::Application::headless().with_assets(TelegramAssetSource::new()).run(...)`, instantiate `cx.new(|cx| AppShell::new_with_client(client.clone(), cx))`, and drive consecutive `shell.update(cx, |s, cx| ...)` closures so `cx.observe(...)` observers flush cleanly between actions.

### 6.2 Hard Regression Guardrails Verified in Phase B
1. **`IconName::ALL.len() == 64` + `IconName::EXTRA` Asset Loading**: Run `HeadlessTranscriptDriver::verify_all_embedded_assets()` AND independently load every new Apple sidebar/hub SVG icon (`hub_books`, `hub_music`, `hub_games`, `hub_appstore`, `hub_creations`, `hub_publish`, `hub_sparkles`) via `TelegramAssetSource::new().load(icon.path())` — asserting `IconName::ALL.len() == 64` remains intact!
2. **`ToolsetId::all().len() == 29`**: Verify all 8 creation & recommendation bot tools remain registered inside existing `ToolsetId::PlatformIm` without altering `ToolsetId::all().len() == 29` (`PROJECT.md § Hard Regression Guardrails`).
3. **`NavRail` & `ChatSidebar` Zero Regression**: Assert `NavRail::items.len() == 9`, `NAV_RAIL_WIDTH == 52.0`, `ChatSidebar::DEFAULT_WIDTH == 288.0`, `COLLAPSED_WIDTH == 64.0`, `ROW_HEIGHT == 62.0`, and `has_leading_app_sidebar == false` default standalone behavior.
4. **Themes & Localization**: Assert `TdesktopThemePalette` (`Night`, `Tinted`, `Day`, `Classic`) and `zh-CN` / `en` locale propagation via `ShellThemeTargets` across `AppleAppSidebar` and `ContentHubWorkspace`.

### 6.3 Tier 1–4 Headless GPUI UI Test Blueprint (`apple_sidebar_ui_e2e.rs`)

| Tier | Test ID | Scope & Concrete `AppShell::inspect_state` / Entity Assertions |
|---|---|---|
| **UI-T1** | `ui_t1_01_apple_sidebar_mounts_and_switches_all_sections` | Switch `AppleAppSidebar` across `Chats`, `Books`, `Music`, `Games`, `AppStore`, `MyCreations`; verify `ActiveWorkspacePanel::ContentHub(section)` in `AppShell::inspect_state(cx)` and non-equal `state_hash` per transition without clearing `ChatSidebar` or `ConversationStage` state. |
| **UI-T1** | `ui_t1_02_apple_sidebar_collapse_expand_and_quick_creations_drawer` | Toggle `AppleAppSidebar` between `204px` expanded and `68px` compact icon rail, and toggle the sidebar-adjacent Quick Creations Drawer (`creations_drawer_open`), verifying cloud creations appear immediately beside the sidebar. |
| **UI-T1** | `ui_t1_03_books_hub_and_book_reader_themes_zoom_bookmarks` | Open `BooksHubView` (`Reading Now`, `Library`, `Book Store`, `My Books`), open a published book in `BookReaderView`, cycle themes (`Light`/`Sepia`/`Gray`/`Night`), scale fonts, navigate chapters, add bookmark, and assert cloud reading progress persists. |
| **UI-T1** | `ui_t1_04_music_hub_synth_and_now_playing_transport_bar` | Open `MusicHubView` (`Home`, `New`, `Radio`, `Library`, `My Music`), synthesize track via `synthesize_music_track_wav()`, start playback in persistent `MusicPlayerBar` (`MusicTransportEngine`), test play/pause/seek/next/repeat/shuffle & synced lyrics without OS audio errors on headless Linux. |
| **UI-T1** | `ui_t1_05_games_hub_and_interactive_mini_game_runtime_sheet` | Open `GamesHubView` (`Discover`, `Arcade`, `Charts`, `Library`, `My Games`), launch a bot-authored game in `GameRuntimeSheet`, feed deterministic `GameInput` + `step(dt_ms)` ticks, verify live score increment & leaderboard submission. |
| **UI-T1** | `ui_t1_06_app_store_hub_unified_storefront` | Open `AppStoreHubView` (`Discover`, `Create`, `Work`, `Play`, `Develop`, `Categories`, `Installed`), verify Today hero cards and unified display of published `App` creations + published `mini_apps` + featured Books/Music/Games. |
| **UI-T1** | `ui_t1_07_publish_review_modal_validation_and_submit` | Open `PublishReviewModal` ("推送审核上架") from `MyCreationsView` / Quick Drawer, verify live `validate_creation_for_review()` error feedback on invalid draft and clean submission (`Draft -> PendingReview`) on valid draft. |
| **UI-T2** | `ui_t2_01_ime_input_isolation_in_hub_search_and_review_modal` | Focus search inputs in Books/Music/Games/AppStore and `PublishReviewModal` text fields; commit CJK + ASCII text via `shell_input_ime.rs` and assert text routes ONLY to active hub input (zero leakage to `CanonicalComposer`). |
| **UI-T2** | `ui_t2_02_escape_modal_priority_and_unauthenticated_guard` | Verify `Escape` closes `PublishReviewModal` before `ContextInspector`; verify unauthenticated shell hides `AppleAppSidebar` and ignores `tg://apple/*` deep-links safely. |
| **UI-T2** | `ui_t2_03_account_switch_and_logout_wipes_private_creations_cache` | Authenticate User A (loads private drafts into `LocalStore` `user_creations_cache`), switch to User B / logout -> assert `MusicTransportEngine` stops, `active_panel` resets to `Chats`, and User A's private creations cache is completely wiped (`LocalStore::bind_account_scope`). |
| **UI-T2** | `ui_t2_04_icon_and_nav_rail_regression_guardrails` | Assert `IconName::ALL.len() == 64`, all `IconName::EXTRA` hub SVGs load cleanly, `NavRail::items.len() == 9`, and `ChatSidebar` default constants remain untouched. |
| **UI-T3** | `ui_t3_01_chat_generative_card_deep_link_opens_target_apple_vertical` | Click a `GenerativeComponent::Button` (`action_payload: "tg://apple/books/<id>?action=read"`) in `ConversationStage` -> `execute_hermes_inline_callback` -> `AppShell::handle_apple_deep_link` -> verify shell switches to `ContentHub(Books)` with `<id>` opened. |
| **UI-T3** | `ui_t3_02_inline_keyboard_url_and_richtext_openurl_deep_link` | Trigger `InlineButtonType::Url("tg://apple/music/<id>")` via `handle_stage_bot_button` AND `RichTextAction::OpenUrl("tg://apple/games/<id>")` AND legacy `creation:open:app:<id>` -> verify all 3 paths route cleanly without external browser fallback. |
| **UI-T3** | `ui_t3_03_ws_events_live_refresh_my_creations_and_public_catalog` | While headless `AppShell` is mounted, trigger creation/publish/unpublish via live server HTTP -> verify WS `UserCreationChanged` and `PublicCatalogChanged` update `ContentHubWorkspace` state live. |
| **UI-T4** | `ui_t4_01_full_ui_journey_bot_create_to_sidebar_creations_publish_and_discover` | End-to-end headless `AppShell` + live `EphemeralTestServer`: User A creates book via `@CreatorStudioBot` -> opens sidebar Quick Creations Drawer -> submits via `PublishReviewModal` -> admin approves -> User B clicks bot recommendation card deep-link -> opens directly in `BookReaderView` -> persists reading progress. |
| **UI-T4** | `ui_t4_02_full_ui_journey_music_and_game_launch_from_app_store_and_chat` | End-to-end headless `AppShell` + live `EphemeralTestServer`: launch published Music & Game from both `AppStoreHubView` and `@CreatorStudioBot /recommend` cards -> verify playback transport, interactive game canvas ticks/score, and seamless return to `Chats`. |

---

## 7. Build Hygiene (`AGENTS.md`) & Gate Attestation

- Root `Cargo.toml` profiles (`dev`, `test`, `ci`, `release`) MUST retain `debug = 0`, `strip = "debuginfo"`, `incremental = false`.
- NEVER create custom `--target-dir` directories.
- Execute `python3 scripts/clean_build_garbage.py` immediately after EVERY `cargo check` / `cargo build` / `cargo test`.
- `TEST_READY.md` is intentionally **NOT** published in Phase A; it will be issued only in Phase B after M6 + `apple_sidebar_ui_e2e.rs` pass full 5-agent verification.
