# Project: Apple-Style Sidebar & Creative Content Ecosystem (`gpui-telegram`)

- **Authoritative Request**: `.agents/teamwork/ORIGINAL_REQUEST.md` (`## Follow-up — 2026-11-24T08:43:16Z`)
- **Workspace Root**: `/home/antigravity/bhrumom/gpui-telegram`
- **Build Hygiene Mandate**: `/home/antigravity/bhrumom/gpui-telegram/AGENTS.md`
- **Survey Evidence**:
  - `.agents/teamwork/explorer_apple_survey_1/handoff.md` (Workspace, GPUI Shell, Styling & Guardrails)
  - `.agents/teamwork/explorer_apple_survey_2/handoff.md` (Cloud Storage, Migration V41, FTS5/CJK Catalog & Data Contracts)
  - `.agents/teamwork/explorer_apple_survey_3/handoff.md` (Bot Subsystem, Media/Reader/Game Engines, Deep-Links & Testing)

---

## Architecture

### System Overview & Data Flow
1. **Apple-Style Primary Sidebar (`AppleAppSidebar`) + Content Hub (`ContentHubWorkspace`)**:
   - Mounted as the leftmost pane in `AppShell::render` (`crates/gpui_telegram_desktop/src/views/shell.rs`) alongside existing Telegram views without altering `NavRail` or `ChatSidebar` contracts.
   - Provides macOS Apple HIG grouped sidebar navigation: **Chats (消息)**, **Books (图书)**, **Music (音乐)**, **Games (游戏)**, **App Store**, plus direct sidebar & sidebar-adjacent access to **My Creations (我的创作 / 云端作品)** (both a full workspace tab AND a collapsible Quick Creations Drawer immediately beside the sidebar).
   - Switching to any Apple section sets `AppShell::active_panel = ActiveWorkspacePanel::ContentHub(ContentHubSection)` (`Books | Music | Games | AppStore | MyCreations`), mounting `ContentHubWorkspace` cleanly without clearing `ChatSidebar` or `ConversationStage` state.
2. **User Cloud Creations, Review/Publishing FSM & Physically Isolated Public Catalog**:
   - Server SQLite **Migration V42** (V41 = upstream automation apps) (`crates/gpui_telegram_server/src/db/creations_store.rs`, pinned literal version `42` in `db/migrations.rs` `extra_migrations`) stores authoritative user cloud data in `user_creations` (monotonic per-user `cloud_revision`, `expected_revision` optimistic locking, soft-delete tombstones, frozen `published_snapshot_json` live-edit isolation).
   - **Review & Publishing FSM**: `Draft -> PendingReview -> Published | Rejected`, `PendingReview -> Draft`, `Published -> Unpublished`, `Rejected -> Draft/PendingReview`. Submission runs `validate_creation_for_review()`, logs immutable rows in `creation_review_audit`, and upon approval with `visibility == Public` atomically upserts into the physically separate `public_catalog_entries` table (trigger-synced to `public_catalog_fts` SQLite FTS5 + CJK substring/bigram search) and `creation_public_assets` (authorizing public CAS media in `user_can_access_media`). Unpublish/delete immediately removes rows from both `public_catalog_entries` and `creation_public_assets` inside one `BEGIN IMMEDIATE` transaction.
   - **4-Layer Visibility Isolation**: Physical table separation + atomic trigger index sync + separate `PublicCatalogService` + bot tool permission boundaries guarantee `Draft`, `PendingReview`, `Rejected`, `Unpublished`, and `Unlisted` items NEVER leak into public sections or bot recommendations.
3. **Bot Creation & Public Recommendation Engine**:
   - 8 bot tools registered under existing `ToolsetId::PlatformIm` (preserving `ToolsetId::all().len() == 29`): `create_book`, `create_music`, `create_game`, `update_my_creation`, `list_my_creations`, `submit_creation_for_review`, `search_public_catalog`, `recommend_public_creations`.
   - Implemented in `ServerPlatformImBridge` AND paired with a seeded official **`@CreatorStudioBot`** (`UID 777_020`, `is_official = true`, `is_inline = true`) supporting deterministic slash/NL commands (`/newbook`, `/newmusic`, `/newgame`, `/mycreations`, `/editwork`, `/submitwork`, `/recommend`) plus `GET /api/v1/bots/inline-query` & `answerInlineQuery` so both LLM-backed and offline/E2E bots work deterministically.
   - Bot replies render `GenerativeCard` + `InlineKeyboardButton` messages carrying canonical `tg://apple/<section>/<creation_id>` deep-links routed by `AppShell::handle_apple_deep_link` straight into the target Apple section & detail/reader/player/game view.
4. **Authentic Cross-Platform Media, Reader & Playable Game Engines**:
   - **Apple Books**: `BookReaderView` with multi-chapter TOC, Light/Sepia/Gray/Night themes, serif/sans/mono font & scale controls, bookmarks, and cloud progress sync (`user_creation_states`).
   - **Apple Music**: Pure-Rust `synthesize_music_track_wav()` (`gpui_telegram_media::media::audio::synth`) generating real 44.1kHz PCM/RIFF WAV + 100-bar 5-bit waveform + deterministic seekable `MusicTransportEngine` (queue, repeat, shuffle, seek, synced lyrics, best-effort OS audio sink on macOS/Linux + silent deterministic headless mode).
   - **Apple Games / Arcade**: Pure-Rust deterministic `MiniGameRuntime` (`GridPuzzle`, `TriviaQuiz`, `InteractiveFiction`, `ArcadeDodge`/`SnakeGrid`, `CardMatch`, plus `MiniAppHtml` via `BotAttachWebView`) rendered interactively via GPUI `canvas()` + keyboard/mouse handlers with persistent cloud leaderboards (`creation_game_scores`).
   - **Apple App Store**: Unified storefront aggregating published `App` creations, existing published `mini_apps` (`is_published = 1`), and featured published Books/Music/Games with Today hero cards, categories, ratings/reviews, and install/open/launch actions.
5. **Client Cloud Sync & Account-Scoped Offline Mirror**:
   - `gpui_telegram_client` provides typed `api_creations.rs` methods, WS `ClientDomainEvent::{UserCreationChanged, PublicCatalogChanged}`, and local SQLite cache tables (`user_creations_cache`, `user_creation_states_cache`, `public_catalog_cache`) automatically wiped by `LocalStore::bind_account_scope` on account/server switch.

---

## Feature Inventory
Every Survey feature is assigned a Milestone (zero unassigned).

| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| F1 | Shared Creation, Catalog & Review Contracts | Strongly typed DTOs (`CreationKind`, `CreationReviewStatus`, `CreationVisibility`, `BookPayload`, `MusicPayload`, `GamePayload`, `AppStoreListingPayload`, `UserCreationDto`, `PublicCatalogItemDto`, `BotRecommendationItem`), `validate_creation_for_review()`, WS `UpdateEvent`s | M1 | survey_1/2/3 |
| F2 | Canonical Apple Hub Deep-Link Router | Parse & route `tg://apple/<books\|music\|games\|appstore\|creations>[/<id>][?tab=...&action=...]` and `creation:open:<kind>:<id>` in `gpui_telegram_core` | M1 | survey_1/3 |
| F3 | Pure-Rust Music Synthesizer & Seekable Transport | `synthesize_music_track_wav()` (PCM/WAV + 5-bit waveform) & cross-platform/headless-safe seekable `MusicTransportEngine` with queue/repeat/shuffle/lyrics | M1 | survey_2/3 |
| F4 | Server Cloud Creations Store & Delta Sync (V42) | Pinned Migration V42 (`extra_migrations` literal `39/40/41/42`), 8 `STRICT` tables + FTS5 triggers, per-user monotonic `cloud_revision`, `expected_revision` 409 CAS, soft-delete tombstones, `user_can_access_media` public asset allowlist | M2 | survey_2/3 |
| F5 | Content Review & Publishing FSM | Strict `Draft -> PendingReview -> Published \| Rejected` + `Unpublished`, live-edit frozen published snapshot isolation, immutable `creation_review_audit`, review queue & approve/reject REST APIs, WS broadcasts | M2 | survey_1/2/3 |
| F6 | Public Catalog Search, Vertical Feeds & App Store Merge | Isolated `public_catalog_entries` + FTS5/CJK search & composite ranker, `/api/v1/catalog/{books,music,games,appstore,recommend}`, App Store merge with `mini_apps`, user states, ratings/reviews, game leaderboards | M2 | survey_2/3 |
| F7 | Bot-Assisted Creation (`@CreatorStudioBot` & Hermes Tools) | 8 tools under `ToolsetId::PlatformIm` (29-toolset invariant preserved), `ServerPlatformImBridge` impls, official `@CreatorStudioBot` (`UID 777_020`) deterministic slash/NL creation + cloud save + review submit + interactive cards | M3 | survey_1/2/3 |
| F8 | Bot Public Content Recommendation & Inline Query Engine | Published+public-only `search_public_catalog` / `recommend_public_creations` / `/recommend` / `GET /api/v1/bots/inline-query` / `answerInlineQuery` returning deep-linked recommendation cards with 4-layer zero-leak enforcement | M3 | survey_1/2/3 |
| F9 | Client Cloud API, WS Events & Account-Scoped Cache | `gpui_telegram_client::net::api_creations`, WS `ClientDomainEvent` propagation, local SQLite cache tables wiped on `LocalStore::bind_account_scope` / `clear_all()` | M4 | survey_2/3 |
| F10 | Deterministic Playable Mini-Game Runtime | Pure-Rust `MiniGameRuntime` (`GridPuzzle`, `TriviaQuiz`, `InteractiveFiction`, `ArcadeDodge`/`SnakeGrid`, `CardMatch`, `MiniAppHtml` hook) with `step(dt_ms)`, `apply_input`, scoring & achievements | M4 | survey_2/3 |
| F11 | Apple-Style Primary Sidebar & Zero-Regression Shell Mount | `AppleAppSidebar` (`204px`/`68px`, Apple HIG vibrancy, `IconName::EXTRA` icons preserving `IconName::ALL == 64`), `ActiveWorkspacePanel::ContentHub`, `ChatSidebar` `has_leading_app_sidebar`, `shell_input_ime.rs`, `ShellThemeTargets`, `semantic_tree.rs` | M5 | survey_1/3 |
| F12 | Sidebar-Adjacent Cloud Creations Hub & Publish Review Modal | Full `MyCreationsView` + collapsible sidebar-adjacent Quick Creations Drawer, status badges, preview/edit/submit, "推送审核上架" modal with live validation & reviewer queue panel | M5 | survey_1/2 |
| F13 | Unified Chat-to-Hub Deep-Link Navigation | `AppShell::handle_apple_deep_link` wired to `handle_deep_link`, `handle_stage_bot_button`, `execute_hermes_inline_callback`, and `RichTextAction::OpenUrl` | M5 | survey_1/3 |
| F14 | Apple Books Section & Embedded Book Reader | `BooksHubView` (Reading Now, Library, Book Store, My Books, 2:3 cover grid, Detail) + `BookReaderView` (TOC, Light/Sepia/Gray/Night themes, fonts/zoom, bookmarks, cloud reading progress) | M6 | survey_1/2/3 |
| F15 | Apple Music Section & Now-Playing Transport Bar | `MusicHubView` (Home, New, Radio, Library, My Music, Charts, Lyrics) + persistent `MusicPlayerBar` wired to `MusicTransportEngine` + `synthesize_music_track_wav` | M6 | survey_1/2/3 |
| F16 | Apple Games / Arcade Section & Interactive Game Launcher | `GamesHubView` (Discover, Arcade, Charts, Library, My Games, Hero Banner, Detail + Leaderboards) + `GameRuntimeSheet` (`MiniGameRuntime` on GPUI `canvas()` + `BotAttachWebView` fallback) | M6 | survey_1/2/3 |
| F17 | Apple App Store Section | `AppStoreHubView` (Today/Discover hero cards, Create/Work/Play/Develop/Categories/Installed, 3-row shelves, unified Mini Apps + Bots + cross-vertical published creations, Detail + Get/Open/Launch) | M6 | survey_1/2/3 |
| F18 | Opaque-Box 4-Tier E2E Suite & Tier 5 Adversarial Hardening | Requirement-driven Tiers 1–4 E2E suite (`TEST_INFRA_APPLE_SIDEBAR.md` -> `TEST_READY.md`) + Phase 2 Tier 5 white-box adversarial hardening + full workspace regression & ledger closure | M7 / E2E | survey_1/2/3 |

---

## Milestones

| # | Name | Scope (Features) | Dependencies | Status |
|---|------|------------------|--------------|--------|
| E2E | E2E Testing Track | Requirement-driven opaque-box 4-tier E2E suite (`TEST_INFRA_APPLE_SIDEBAR.md`, HTTP/WS Tiers 1–4 + GPUI headless UI E2E after M6 -> `TEST_READY.md`) | none (UI binary after M6) | PHASE_A_DONE (`5/5 PASS`, 94 tests in `tests/e2e_suite/tests/apple_sidebar_*`; Phase B pending M6) |
| M1 | Shared Contracts, Deep-Links & Audio Synth | F1, F2, F3 (`gpui_telegram_proto`, `gpui_telegram_core`, `gpui_telegram_media`) | none | DONE (`5/5 PASS`, 404/404 crate tests + 401/401 probes, `sub_orch_m1/handoff.md`) |
| M2 | Server Cloud Creations, Review FSM & Public Catalog | F4, F5, F6 (`gpui_telegram_server` DB, search, REST/WS routes) | M1 | DONE (`5/5 PASS`, 12/12 M2 + 48/48 lib + 140+ live probes, `sub_orch_m2/handoff.md`) |
| M3 | Bot Creation & Public Recommendation Engine | F7, F8 (`gpui_telegram_agent` tools + `gpui_telegram_server` bridge, `@CreatorStudioBot`, inline query) | M2 | DONE (`5/5 PASS`, 82 agent + 16 M3 + 12 M2 + 48 server + 4 hermes + 999 probes + 500 fuzz, `sub_orch_m3_gen3/handoff.md`) |
| M4 | Client Cloud Sync, Local Mirror & Mini-Game Runtime | F9, F10 (`gpui_telegram_client` net/cache + `gpui_telegram_desktop` `game_runtime.rs`) | M2 (serialized after M3) | DONE (`5/5 PASS`, 37+8+12+8+52 tests + 328 F9 probes + 18,000 F10 fuzz ops, `sub_orch_m4_gen6/handoff.md`) |
| M5 | Apple Sidebar, Cloud Creations Hub & Shell Integration | F11, F12, F13 (`gpui_telegram_desktop` icons, `AppleAppSidebar`, `ContentHubWorkspace`, `MyCreations`, `PublishReviewModal`, `AppShell` routing/IME/deep-links) | M3, M4 | IN_PROGRESS (`c96ab87fb98f90cb0ad7b45745ea9fcc`, `sub_orch_m5_gen9` — Iter 8 Batch 1 `3/3 CLEAN/APPROVE/APPROVE`; running Batch 2 -> 5/5 Gate) |
| M6 | Four Apple Verticals (Books, Music, Games, App Store) | F14, F15, F16, F17 (`gpui_telegram_desktop` `books_view`, `book_reader`, `music_view`, `music_player_bar`, `games_view`, `game_runtime_sheet`, `app_store_view`) | M5 | PLANNED |
| M7 | Final Milestone: 100% E2E Pass (Tiers 1–4) + Tier 5 Hardening | F18 (Phase 1: Tiers 1–4 sequential pass; Phase 2: Tier 5 white-box adversarial hardening + full regression/ledger gate) | M6, E2E | PLANNED |

---

## Interface Contracts

### 1. Shared Proto Contracts (`crates/gpui_telegram_proto/src/creations.rs` & `sync/events.rs`)
- **Enums** (`#[serde(rename_all = "snake_case")]`):
  - `CreationKind`: `Book | Music | Game | App`
  - `CreationReviewStatus`: `Draft | PendingReview | Published | Rejected | Unpublished`
  - `CreationVisibility`: `Private | Unlisted | Public`
  - `CatalogSortMode`: `Featured | Popular | TopRated | Newest`
  - `ReviewDecision`: `Approve | Reject`
- **Core Envelope (`UserCreationDto`)**:
  - `creation_id: String` (`cr_<uuid>`), `owner_user_id: i64`, `creator_bot_user_id: Option<i64>`, `creator_chat_id: Option<i64>`, `kind: CreationKind`
  - `title`, `subtitle`, `summary`, `author_display_name`, `language`, `genres: Vec<String>`, `tags: Vec<String>`, `cover: CoverArtwork`, `age_rating`
  - `payload: CreationPayload` (`#[serde(tag = "kind", rename_all = "snake_case")]`: `Book(BookPayload) | Music(MusicPayload) | Game(GamePayload) | App(AppStoreListingPayload)`)
  - `version: u32`, `cloud_revision: i64`, `review_status: CreationReviewStatus`, `visibility: CreationVisibility`, `has_live_published_version: bool`, `published_version: Option<u32>`, `rejection_reason: Option<String>`, `review_notes: Option<String>`
  - Timestamps (`submitted_at_ms`, `reviewed_at_ms`, `published_at_ms`, `created_at_ms`, `updated_at_ms`, `deleted_at_ms`) + metrics (`rating_avg: f32`, `rating_count: u32`, `engagement_count: u64`, `save_or_install_count: u64`)
- **Payloads**:
  - `BookPayload`: `author`, `series_name`, `series_index`, `blurb`, `estimated_read_mins`, `word_count`, `chapters: Vec<BookChapter>` (`pages: Vec<BookPage>` with `BookRichBlock::{Heading, Paragraph, BlockQuote, Callout, Illustration, Divider}`).
  - `MusicPayload`: `artist`, `album_title`, `release_year`, `primary_genre`, `mood_tags`, `bpm` (`30..=300`), `musical_key`, `time_signature`, `total_duration_secs`, `tracks: Vec<MusicTrack>` (`instruments`, `melody_notes`, `chord_progression`, `lyrics: Vec<SyncedLyricLine>`, `audio_media_file_id`, `waveform_5bit`).
  - `GamePayload`: `developer`, `category`, `controls_hint`, `rules_markdown`, `engine_kind`, `supports_leaderboard`, `estimated_session_mins`, `spec: GameSpec` (`GridPuzzle | TriviaQuiz | InteractiveFiction | ArcadeDodge | SnakeGrid | CardMatch | MiniAppHtml`), `achievements: Vec<GameAchievementDef>`.
  - `AppStoreListingPayload`: `developer_name`, `version_string`, `release_notes`, `category`, `long_description`, `screenshots`, `size_label`, `price_label`, `permissions`, `linked_miniapp`, `ui_schema_json`.
- **Validator**: `pub fn validate_creation_for_review(creation: &UserCreationDto) -> Result<(), Vec<String>>` (enforces title/summary/author bounds, <=4 MiB payload, hex colors, no `<script`/`javascript:`, per-kind structural playability/readability checks).
- **Realtime WS Events (`proto/src/sync/events.rs`)**:
  - `UpdateEvent::UserCreationChanged { creation_id: String, owner_user_id: i64, kind: CreationKind, review_status: CreationReviewStatus, cloud_revision: i64 }`
  - `UpdateEvent::PublicCatalogChanged { creation_id: String, kind: CreationKind, action: String /* published|updated|removed */ }`

### 2. Deep-Link Contract (`crates/gpui_telegram_core/src/core/deep_links/`)
- Canonical URI: `tg://apple/<books|music|games|appstore|creations>[/<creation_id>][?tab=discover|my_creations&action=open|read|play|install]`
- Legacy/callback alias also accepted: `creation:open:<book|music|game|app>:<creation_id>`
- Parser: `pub fn parse_apple_deep_link(uri: &str) -> Option<AppleDeepLink>` returning `{ section: AppleDeepLinkSection, creation_id: Option<String>, tab: Option<String>, action: Option<String> }`.

### 3. Server REST & Bot API Contract (`gpui_telegram_server`)
- **User Cloud Creations (Bearer auth, strict `owner_user_id` isolation)**:
  - `POST /api/v1/creations` -> `201`/`200` `UserCreationDto`
  - `GET /api/v1/creations/my?kind=&status=&q=` -> `{ items: Vec<UserCreationDto>, latest_revision: i64 }`
  - `GET /api/v1/creations/sync?since_revision=N` -> `{ latest_revision: i64, items: Vec<UserCreationDto> }` (includes soft-deleted tombstones)
  - `GET /api/v1/creations/{id}` -> owner sees draft+status; non-owner sees ONLY frozen published snapshot if `Published && (Public|Unlisted)`, else `404`/`403`
  - `PUT /api/v1/creations/{id}` -> body `{ ..., expected_revision: Option<i64> }` (`409 revision_conflict` on stale revision; live-edit preserves frozen `published_snapshot_json` until re-approved)
  - `DELETE /api/v1/creations/{id}` -> soft-delete tombstone + immediate atomic purge from `public_catalog_entries` & `creation_public_assets`
  - `POST /api/v1/creations/{id}/submit-review` -> runs `validate_creation_for_review()` (`422` + `validation_errors` on failure) -> `PendingReview`
  - `POST /api/v1/creations/{id}/withdraw-review` -> `PendingReview -> Draft`
  - `POST /api/v1/creations/{id}/unpublish` -> `Published -> Unpublished` (immediate atomic removal from `public_catalog_entries` & `public_catalog_fts`)
  - `GET /api/v1/creations/{id}/audit` -> `Vec<CreationReviewAuditDto>`
- **Moderation / Review Queue (Bearer auth; supports admin `users.is_support == true` & explicit local review policy without skipping `PendingReview` or audit logging)**:
  - `GET /api/v1/creations/review-queue?status=pending_review&kind=` -> `{ items: Vec<UserCreationDto> }`
  - `POST /api/v1/creations/{id}/review` -> body `{ decision: "approve"|"reject", visibility: Option<"public"|"unlisted">, reason: Option<String>, notes: Option<String> }`
- **Public Catalog & Vertical Discovery (Bearer auth; STRICTLY `public_catalog_entries` only — zero private/draft/pending/rejected/unpublished/unlisted rows)**:
  - `GET /api/v1/catalog?kind=all|book|music|game|app&q=&genre=&sort=featured|popular|top_rated|newest&limit=&offset=`
  - `GET /api/v1/catalog/books` | `/music` | `/games` | `/appstore` (`/appstore` aggregates published `App` creations, `mini_apps WHERE is_published = 1`, and cross-vertical featured published Books/Music/Games)
  - `GET /api/v1/catalog/recommend?q=&kind=&genre=&limit=` -> `{ items: Vec<BotRecommendationItem> }` (each with `deep_link: "tg://apple/<section>/<creation_id>"`)
  - `PUT /api/v1/catalog/{id}/state` & `GET /api/v1/catalog/my-states` -> per-user reading progress/bookmarks, music loved/position, game save state/achievements, app install state
  - `POST /api/v1/catalog/{id}/reviews` & `GET /api/v1/catalog/{id}/reviews` -> ratings (`1..=5`) & written reviews (published items only)
  - `POST /api/v1/catalog/{id}/scores` & `GET /api/v1/catalog/{id}/leaderboard` -> game high scores (published games only)
- **Bot Creation & Recommendation Interfaces**:
  - Hermes `ToolsetId::PlatformIm` tools (`ServerPlatformImBridge`): `create_book`, `create_music`, `create_game`, `update_my_creation`, `list_my_creations`, `submit_creation_for_review`, `search_public_catalog`, `recommend_public_creations`.
  - Official `@CreatorStudioBot` (`UID 777_020`, `is_official = true`, `is_inline = true`): deterministic commands `/start`, `/help`, `/newbook`, `/newmusic`, `/newgame`, `/mycreations`, `/editwork`, `/submitwork`, `/recommend` + NL creation fallback, replying with rich `GenerativeCard` + `InlineKeyboardButton` deep-links.
  - Inline Bot Query: `GET /api/v1/bots/inline-query?bot_username=&query=&kind=&limit=` + `answerInlineQuery` in `telegram_bot_api.rs` (returns ONLY published+public items).

### 4. Desktop Shell & UI Integration Contract (`gpui_telegram_desktop`)
- `ActiveWorkspacePanel::ContentHub(ContentHubSection)` where `ContentHubSection` is `Books | Music | Games | AppStore | MyCreations`.
- Leftmost `AppleAppSidebar` (`204px` expanded / `68px` collapsed, macOS `52px` drag clearance) mounted beside `ChatSidebar` (when `Chats`) or `ContentHubWorkspace` (when `ContentHub`), plus toggleable sidebar-adjacent Quick Creations Drawer.
- `ChatSidebar` gets opt-in `has_leading_app_sidebar: bool` (default `false` so existing standalone tests remain 100% unchanged).
- `AppShell::handle_apple_deep_link(&mut self, uri: &str, cx: &mut Context<Self>) -> bool` wired into `handle_deep_link`, `handle_stage_bot_button`, `execute_hermes_inline_callback`, and `RichTextAction::OpenUrl`.

---

## Code Layout

```text
crates/gpui_telegram_core/src/core/deep_links/
  └── apple_deep_link.rs                # M1: tg://apple/* & creation:open:* parser + router hook
crates/gpui_telegram_proto/src/
  ├── creations.rs                      # M1: DTOs, payloads, review FSM types, validator
  └── sync/events.rs                    # M1: UpdateEvent::{UserCreationChanged, PublicCatalogChanged}
crates/gpui_telegram_media/src/media/audio/
  ├── synth.rs                          # M1: MusicTrack -> PCM/WAV + 5-bit waveform synthesizer
  └── music_player.rs                   # M1: Cross-platform headless-safe seekable MusicTransportEngine
crates/gpui_telegram_server/src/
  ├── db/creations_store.rs             # M2: Migration V42 + cloud CRUD, FSM, audit, states, scores
  ├── db/migrations.rs                  # M2: Pin extra_migrations literal 39, 40, 41
  ├── db/social_store.rs                # M2: user_can_access_media public creation asset check
  ├── search/catalog_index.rs           # M2: Isolated FTS5 + CJK search & recommendation ranker
  ├── ws/gateway_hub.rs                 # M2: Add broadcast_to_all for PublicCatalogChanged WS push
  ├── routes/creations.rs               # M2: /api/v1/creations/* & /api/v1/catalog/* Axum routes
  ├── routes/creator_bot.rs             # M3: @CreatorStudioBot + inline query endpoint
  ├── routes/bots_miniapps.rs           # M3: Route @CreatorStudioBot (UID 777_020)
  ├── routes/telegram_bot_api.rs        # M3: answerInlineQuery support
  └── hermes_im_bridge.rs               # M3: 8 PlatformIm creation & public-recommendation tools
crates/gpui_telegram_agent/src/hermes/
  └── toolsets.rs                       # M3: Register 8 tools inside existing ToolsetId::PlatformIm
crates/gpui_telegram_client/src/
  ├── net/api_creations.rs              # M4: Typed HttpClient/TelegramClient creation & catalog APIs
  ├── cache/sqlite_cache.rs             # M4: Local mirror tables (creations, states, catalog)
  ├── cache/local_store.rs              # M4: Account-scoped clear_all() wipe on account switch
  └── client_dispatch.rs                # M4: ClientDomainEvent::{UserCreationChanged, PublicCatalogChanged}
crates/gpui_telegram_desktop/
  ├── assets/icons/                     # M5: hub_{books,music,games,appstore,creations,publish,sparkles}.svg
  ├── src/theme/assets_embedded.rs      # M5: Add new icons ONLY in IconName::EXTRA (ALL stays 64)
  ├── src/views/apple_sidebar.rs        # M5: Apple-style primary sidebar (`AppleAppSidebar`)
  ├── src/views/content_hub/
  │   ├── game_runtime.rs               # M4: Pure-Rust deterministic MiniGameRuntime
  │   ├── mod.rs                        # M5: ContentHubWorkspace entity & shared sync state
  │   ├── apple_primitives.rs           # M5: Apple HIG hero banners, cover cards, pills, badges
  │   ├── creations_view.rs             # M5: My Creations workspace + sidebar-adjacent quick drawer
  │   ├── publish_review_modal.rs       # M5: "推送审核上架" modal + reviewer queue panel
  │   ├── books_view.rs                 # M6: Apple Books UI
  │   ├── book_reader.rs                # M6: Embedded Apple Books reader
  │   ├── music_view.rs                 # M6: Apple Music UI
  │   ├── music_player_bar.rs           # M6: Persistent Apple Music Now-Playing transport bar
  │   ├── games_view.rs                 # M6: Apple Games / Arcade UI
  │   ├── game_runtime_sheet.rs         # M6: Interactive GPUI game canvas + BotAttachWebView launcher
  │   └── app_store_view.rs             # M6: Apple App Store unified storefront UI
  └── src/views/app_shell/
      ├── shell_content_hub.rs          # M5: AppShell hub sync, deep-link & playback coordinator
      ├── shell_input_ime.rs            # M5/M6: Register content hub & modal inputs
      ├── shell_theme.rs                # M5: Propagate theme & locale to apple_sidebar + content_hub
      └── semantic_tree.rs              # M5: Map ActiveWorkspacePanel::ContentHub(_)
tests/e2e_suite/tests/
  ├── apple_sidebar_tier1_feature.rs    # E2E: Tier 1 isolated feature coverage (>=5/feature)
  ├── apple_sidebar_tier2_boundary.rs   # E2E: Tier 2 boundaries, security & zero-leak isolation
  ├── apple_sidebar_tier3_cross_feature.rs # E2E: Tier 3 pairwise cross-feature interactions
  ├── apple_sidebar_tier4_scenarios.rs  # E2E: Tier 4 real-world journeys & server restart durability
  └── apple_sidebar_ui_e2e.rs           # E2E: Headless GPUI Apple sidebar, verticals & deep-link E2E
```

---

## Hard Regression Guardrails & Workspace Mandates (NON-NEGOTIABLE)
1. **`IconName::ALL.len() == 64`**: Add ALL new SVG icons exclusively to `IconName::EXTRA` (`assets_embedded.rs`).
2. **`ToolsetId::all().len() == 29`**: Register all 8 new bot tools inside existing `ToolsetId::PlatformIm` (`toolsets.rs`).
3. **`NavRail` & `NavDestination` Untouched**: Keep `NavRail::items.len() == 9`, `NAV_RAIL_WIDTH == 52.0`, and all 9 `NavDestination` variants unchanged. Preserve `shell_shortcuts.rs` & `shell.rs` assertions checked by `milestone_1_nav_calls_regression_tests` and `challenger_m3_settings_nav_stress_tests`.
4. **`ChatSidebar` Truthful-Empty & Geometry**: Keep `DEFAULT_WIDTH == 288.0`, `COLLAPSED_WIDTH == 64.0`, `ROW_HEIGHT == 62.0`, and empty default state (`has_leading_app_sidebar` defaults to `false`).
5. **Desktop `[lib] test = false`**: Place ALL desktop tests in `crates/gpui_telegram_desktop/tests/<suite>.rs`.
6. **Migration V41 Safety**: Never append to `ALL_MIGRATIONS` (`schema.rs`, length 38); pin literal versions `39`, `40`, `41` in `db/migrations.rs` `extra_migrations`.
7. **Build Hygiene (`AGENTS.md`)**: Keep `debug = 0`, `strip = "debuginfo"`, `incremental = false` in root `Cargo.toml`; NEVER create `--target-dir`; run `python3 scripts/clean_build_garbage.py` immediately after EVERY `cargo check`/`build`/`test`; use `tempfile::TempDir` for all test fixtures.
8. **Ledger Closure Gate**: `python3 scripts/verify_ledger_closure.py --ci-without-upstream --skip-cargo` MUST pass (no `#![allow(dead_code)]`, `#![allow(unused)]`, or bare `pub use` shims in ledger-tracked files).
