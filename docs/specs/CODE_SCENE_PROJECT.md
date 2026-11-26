# Project: Code Scene ("代码场景") for `gpui-telegram`

> **Authoritative User Request**: `/home/antigravity/bhrumom/gpui-telegram/.agents/teamwork/ORIGINAL_REQUEST.md` (`## Follow-up — 2026-11-25T08:18:25Z`, lines 17–18)
> **Build Hygiene Policy**: `/home/antigravity/bhrumom/gpui-telegram/AGENTS.md`

## Architecture

Code Scene ("代码场景") adds a first-class left-sidebar entry to `gpui-telegram` that opens an immersive, Douyin-like vertical feed for **code-authored Videos, Animations, and Playable Interactive Games** created by users via AI Bots, driven by a **5-stage Douyin-grade Recommendation Engine** and a **dual-engine AI Bot Creation & Publishing pipeline**.

### Layered Dependency DAG (Zero New External Crates, Zero Cycles)

```
gpui_telegram_core
   └── gpui_telegram_proto  (+ code_scene_contracts.rs: wire DTOs & telemetry contracts)
          ├── gpui_telegram_code_scene  [NEW pure-Rust crate: zero gpui, zero unsafe]
          │      ├── lang/    (CodeSceneScript 'cs1' lexer, parser, compiler, sandboxed bytecode VM, stdlib, budgets)
          │      ├── runtime/ (FrameDrawList, PixelBuffer, Video seek/timeline, Animation, Game input/HUD, dry-run validator)
          │      ├── seed_library.rs (18 high-craft real-code bot creations: 6 Videos, 6 Animations, 6 Games)
          │      ├── reco/    (5-stage Douyin Recommender: Stage 0 telemetry -> 6-ch recall -> multi-task eCPM -> UCB/MMR/scatter -> eval)
          │      ├── feed_session.rs (pre-warmed [i-1,i,i+1] window, live tail re-ranking, tab/search filters)
          │      └── synth/   (Dual-engine synthesizer: offline deterministic procedural generator + live LLM prompt/repair helpers)
          ├── gpui_telegram_agent  (+ Hermes code_scene_* tools, /scene slash command, ProviderRouter LLM + self-repair + offline fallback)
          ├── gpui_telegram_server (+ MIGRATION_V42_CODE_SCENE STRICT WAL DB, REST /api/v1/code-scene/*, live recommender, IM card bridge)
          ├── gpui_telegram_client (+ HTTP client, offline-first SqliteClientCache tables, telemetry outbox, LocalCodeSceneService)
          └── gpui_telegram_desktop
                 ├── views/code_scene/ (Hybrid 60fps GPUI CodeSceneViewport + Douyin snap gestures/HUD + 4 slide-over drawers)
                 ├── views/app_shell/shell_code_scene.rs (full-pane ActiveWorkspacePanel::CodeScene, IME/focus, Chat card interop)
                 └── views/sidebar/ + nav_rail.rs (Expanded & Collapsed left sidebar "代码场景" entry, IconName::EXTRA)
```

### Core Architectural Decisions (Grounded in Step 0 Survey)
1. **Zero New External Crates (`Cargo.lock` Unchanged)**: `/opt/cargo/registry` is not pre-populated and `Cargo.lock` contains zero external scripting/VM crates (`rhai`/`mlua`/`wasmi`/`boa`). All VM, rendering, synthesis, and recommendation components are implemented in 100% pure Rust using existing locked workspace crates (`gpui 0.2.2`, `serde`, `serde_json`, `rusqlite 0.39`, `tantivy 0.22`, `sha2`, `chrono`, `uuid`, `image 0.25`, `axum 0.8`, `reqwest 0.12`, `tokio 1`, `thiserror`).
2. **Hybrid Pure-GPUI 0.2.2 Procedural Stage (Cross-Platform + Floating HUD Safe)**: Native `WKWebView` is macOS-only and breaks floating GPUI HUD overlays. Instead, `CodeSceneViewport` combines Layer 1 bounded BGRA8 `Arc<gpui::RenderImage>` (with mandatory `cx.drop_image(prev, None)` VRAM cleanup), Layer 2 60fps `PathBuilder` + `window.paint_path` + `linear_gradient` vector canvas, and Layer 3 crisp `div()` HUD/caption overlays, pumped strictly on the active visible card via `dedup_request_animation_frame`.
3. **100% Genuine Code Execution**: Every seeded creation and every AI-bot-generated creation is stored as inspectable, editable `CodeSceneScript` (`cs1`) source code and runs through the exact same sandboxed bytecode VM — ZERO hardcoded fake Rust renderers.
4. **Offline-First + Live Server Parity**: `LocalCodeSceneService` + `SqliteClientCache` make the full Douyin feed, 18 real-code assets, live recommender, social state, and AI Bot studio work instantaneously even offline/local, while seamlessly syncing with `gpui_telegram_server` (`V42` SQLite + REST + live `ProviderRouter` LLM) when connected.

---

## Feature Inventory

Every feature discovered across Step 0 Surveys (`explorer_cs_survey_1`, `explorer_cs_survey_2`, `explorer_cs_survey_3`) is assigned to a milestone (0 unassigned):

| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | `CS-F01` Shared Code Scene Contracts | Wire & domain DTOs in `gpui_telegram_proto::code_scene_contracts` (`AssetKind`, `CodeSceneAssetDto`, `AssetManifest`, `CodeSceneFeedTab`, `CodeSceneFeedRequest/Response`, `RankedSceneItemDto`, `RecommendationExplanation`, `ScoreBreakdown`, `SceneBehaviorEventDto`, `CodeSceneCommentDto`, `PublishCodeSceneRequest/Response`, `AiGenerateSceneRequest/Response`, `RemixSceneRequest`, `ValidationReportDto`). | M1 | survey_1, survey_2, survey_3 |
| 2 | `CS-F02` `CodeSceneScript` (`cs1`) VM & Sandbox | Pure-Rust zero-`unsafe` lexer, parser, bytecode compiler, deterministic VM, math/easing/keyframe/noise/RNG/color/vector/pixel-shader/collision stdlib, and fail-closed per-frame op/time/stack/heap/draw/pixel budgets + NaN/Inf sanitization. | M1 | survey_1, survey_2, survey_3 |
| 3 | `CS-F03` Video / Animation / Playable Game Execution Modes | Unified `on_init`/`on_update`/`on_render` lifecycle supporting Code Videos (play/pause, `0.5x..2x` speed, timecode, loop count, deterministic `seek_video(t)`), Code Animations (60fps pointer-reactive generative art/shaders), Playable Games (keyboard/mouse input, score/high-score/game-over HUD, restart, `GameSessionTelemetry`), and pre-publish `compile_and_verify` 5-frame dry-run validator. | M1 | survey_1, survey_3 |
| 4 | `CS-F04` Real-Code Seed Asset Library (18 Bot Creations) | 18 visually distinct, 100% real `CodeSceneScript` assets authored by official bots (6 Code Videos, 6 Code Animations, 6 Playable Games) all executed through the real VM. | M1 | survey_1, survey_2, survey_3 |
| 5 | `CS-F05` Behavioral Telemetry & Kind-Normalized Reward Shaping | Stage-0 telemetry capturing dwell time, completion ratio (`0..3.0`), loops, game interaction depth (`inputs`/`play_ms`/`restarts`/`score`), explicit positives (`Like`/`DoubleTapLike`/`Bookmark`/`Comment`/`Share`/`Follow`/`InspectCode`/`Remix`), fast-skip negatives (`<1.5s`, `1.5..3.0s`), and explicit negatives (`NotInterested`/`HideAuthor`/`HideTag`) with kind-normalized reward. | M2 | survey_1, survey_2, survey_3 |
| 6 | `CS-F06` 5-Stage Douyin Recommendation Engine | Stage 1 6-channel recall (Dual-Timescale 64-dim Dense Embedding, Swing Co-Engagement CF, Tag/Kind Affinity, Creator/Bot Graph, 4-Tier Traffic Pool Wilson×Freshness Hot/Velocity, Cold-Start Exploration + hard filtering); Stage 2 Multi-Task eCPM scorer with Bayesian smoothing, rapid real-time dual-timescale online updates (`eta_short=0.35`, `eta_long=0.08`, negative push), and `CreatorJustPublished` fast-track; Stage 3 UCB1/Thompson exploration + greedy MMR (`lambda=0.72`) + Author/Kind/Tag/Cold-Start sliding-window scatter rules + localized `RecommendationExplanation`. | M2 | survey_1, survey_2, survey_3 |
| 7 | `CS-F07` Offline RecSys Eval Harness & Feed Session Controller | Quantitative synthetic-persona benchmark (`reco::eval`: `NDCG@10 >= 0.75` vs baselines, mid-session pivot `<= 2` refreshes, `0` scatter violations, `ILD >= 0.30`, cold-start exposure `<= 10` slots, `0%` dislike re-appearance) + pure-Rust `FeedSessionController` (`[i-1,i,i+1]` preload, tabs/search, swipe-away telemetry flush, live unseen-tail re-ranking). | M2 | survey_1, survey_3 |
| 8 | `CS-F08` Dual-Engine AI Code Synthesizer | Pure-Rust `DeterministicProceduralSynthesizer` (zh-CN/en prompt & remix parser -> 12+ parameterized procedural templates passing `compile_and_verify` offline) + `LlmPromptBuilder`/`LlmResponseParser` for live LLM generation and compiler-diagnostic self-repair. | M2 | survey_2 |
| 9 | `CS-F09` Server SQLite `V42` Store, REST API & Live Recommender | `MIGRATION_V42_CODE_SCENE` (`STRICT` WAL tables for scenes, versions, interactions, likes, bookmarks, comments, follows, user profiles, bot drafts), FK-safe official seed bootstrap, Axum `/api/v1/code-scene/*` endpoints, and live server recommender state. | M3 | survey_2, survey_3 |
| 10 | `CS-F10` AI Bot Creation & Publishing Pipeline | Hermes toolset (`code_scene_create_draft`/`iterate_draft`/`validate`/`publish`/`remix`/`list_mine`) + `/scene` slash command + natural-language chat creation via `@fabushi_ai_bot` / custom bots; `ProviderRouter` live LLM (up to 2 auto-repair iterations) with automatic offline `DeterministicProceduralSynthesizer` fallback; atomic publish + chat `GenerativeCard` with `code_scene:open/remix/share:<id>` buttons. | M3 | survey_1, survey_2, survey_3 |
| 11 | `CS-F11` Client Sync, Offline `SqliteClientCache` & Account Isolation | Typed HTTP client (`gpui_telegram_client`), local SQLite cache + telemetry outbox scoped via `bind_account_scope`, and local-first `LocalCodeSceneService` for seamless offline + server-synced operation. | M3 | survey_2, survey_3 |
| 12 | `CS-F12` Hybrid 60fps GPUI `CodeSceneViewport` & Game Focus Isolation | Layer 1 BGRA8 `Arc<RenderImage>` (`cx.drop_image` leak guard) + Layer 2 `PathBuilder`/`paint_path`/`linear_gradient` vector canvas + Layer 3 `div()` HUD overlay; active-card-only `dedup_request_animation_frame`; `map_point_contain` pointer mapping; explicit Game-vs-Feed input capture & visual audio pulse equalizer. | M4 | survey_1, survey_3 |
| 13 | `CS-F13` Douyin Vertical Snap Feed, Gestures, Floating HUD & Heart Burst | Full-viewport dark cinematic stage, wheel/trackpad inertia & vertical drag-to-swipe snap physics, keyboard shortcuts (`Up/Down/J/K/Space/L/C/M/R/F/Esc`), double-click heart burst animation, floating top tabs (`推荐/关注/视频/动画/游戏/我的作品` + search + `+ AI创作`), right vertical action rail, bottom-left creator/`🤖 @bot`/tags/FPS/reason overlay + draggable timeline scrubber & speed control. | M4 | survey_1, survey_2, survey_3 |
| 14 | `CS-F14` Slide-Over Interactive Drawers (Comments, Code Editor, Creator Sheet, AI Bot Studio) | Translucent drawers above stage: Comments (sort/reply/like/CJK IME), Live Code Inspector & Hot-Reload Editor (`</>` with real-time `compile_and_verify`), Creator/Bot Profile Sheet, and In-Scene AI Bot Creation & Remix Studio (bot picker, modality, prompt/remix, live preview, multi-turn iterate, 1-click publish directly into feed). | M4 | survey_1, survey_2, survey_3 |
| 15 | `CS-F15` Left Sidebar Entry, `AppShell` Full-Pane Mounting & Chat Interoperability | Expanded (`288px`, below `search_bar`) & Collapsed (`64px`, `rail-code-scene-btn` + deferred tooltip) left sidebar `"代码场景"/"Code Scene"` entry + `NavDestination::CodeScene` + `IconName::EXTRA` + Account Popover/Main Menu + `Cmd/Ctrl+Shift+S`; sibling full-pane `ActiveWorkspacePanel::CodeScene` beside `self.sidebar`; focus/CJK IME integration (`shell.rs:1574-1577`, `shell_input_ime.rs`); theme/locale propagation; Chat `GenerativeCard` `code_scene:*` callbacks + Share-to-Chat forwarding. | M5 | survey_1, survey_2, survey_3 |

---

## Milestones

| # | Name | Scope | Dependencies | Status |
|---|------|-------|--------------|--------|
| M1 | Contracts, `CodeSceneScript` VM Runtime & 18-Asset Real-Code Seed Library | `CS-F01`, `CS-F02`, `CS-F03`, `CS-F04` (`gpui_telegram_proto`, new `crates/gpui_telegram_code_scene` lang+runtime+seeds) | none | DONE (`965e621a969896cd1b62a21597a66034`, 5/5 PASS, proto 113/113, code_scene 23/23, 81/81 challenger probes) |
| M2 | 5-Stage Douyin Recommendation Engine, Feed Session & AI Code Synthesizer | `CS-F05`, `CS-F06`, `CS-F07`, `CS-F08` (`crates/gpui_telegram_code_scene` reco+eval+feed_session+synth) | M1 | DONE (`bbeb6dfeaf2716640f5e4b9be6807ad4`, 5/5 PASS, 51/51 tests, NDCG@10=0.8284, HR@5=1.00, pivot=1 refresh, ILD=0.8147) |
| M3 | Server Persistence (`V42`), REST API, Client Local-First Cache & AI Bot Publishing | `CS-F09`, `CS-F10`, `CS-F11` (`gpui_telegram_agent`, `gpui_telegram_server`, `gpui_telegram_client`) | M1, M2 | IN_PROGRESS (`93a43e12254e94104dd23c49ecb4cd6d`) |
| M4 | Hybrid 60fps GPUI `CodeSceneViewport`, Douyin Feed UI & Slide-Over Drawers | `CS-F12`, `CS-F13`, `CS-F14` (`crates/gpui_telegram_desktop/src/views/code_scene/*` + headless GPUI tests) | M1, M2, M3 | PLANNED |
| M5 | Left Sidebar Entry, `AppShell` Mounting, IME/Focus & Chat Interoperability | `CS-F15` (sidebar, `NavRail`, `AppShell`, `shell_code_scene.rs`, `IconName::EXTRA`, chat cards, full regression verification) | M4 | PLANNED |
| E2E | Parallel Opaque-Box E2E Testing Track | Requirement-driven 4-Tier opaque-box E2E harness & test suite (`CODE_SCENE_TEST_INFRA.md` -> `CODE_SCENE_TEST_READY.md`) | Parallel | DONE (`3caf9452cd71a8c43a613156f536940e`, 5/5 PASS, 188 tests T1-T4, 14/14 mutations killed) |
| M6 | Final Milestone: E2E Tiers 1-4 Pass + Tier 5 Adversarial Hardening | Phase 1: pass 100% E2E Tiers 1-4 + repo regression + ledger closure; Phase 2: white-box Challenger-led Tier 5 adversarial hardening | M5, E2E | PLANNED |

---

## Interface Contracts

### 1. `gpui_telegram_proto::code_scene_contracts` ↔ All Crates
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind { Video, Animation, Game }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSceneFeedTab { ForYou, Following, Videos, Animations, Games, MyWorks }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetManifest {
    pub kind: AssetKind,
    pub width: u32,                // clamped 160..=720 (default 360)
    pub height: u32,               // clamped 160..=1280 (default 640)
    pub fps: u32,                  // clamped 15..=60 (default 60)
    pub duration_sec: Option<f64>, // required for Video (e.g., 5.0..=120.0)
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSceneAssetDto {
    pub scene_id: String,
    pub title: String,
    pub description: String,
    pub kind: AssetKind,
    pub tags: Vec<String>,
    pub author_user_id: i64,
    pub author_username: String,
    pub creator_bot_id: Option<i64>,
    pub creator_bot_username: Option<String>,
    pub remix_parent_id: Option<String>,
    pub source_code: String,
    pub code_format: String, // "cs1"
    pub manifest: AssetManifest,
    pub controls_hint: String,
    pub plays_count: u64,
    pub likes_count: u64,
    pub bookmarks_count: u64,
    pub comments_count: u64,
    pub shares_count: u64,
    pub remixes_count: u64,
    pub viewer_liked: bool,
    pub viewer_bookmarked: bool,
    pub viewer_following_author: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub embedding_sim: f64,
    pub swing_cf: f64,
    pub tag_kind_affinity: f64,
    pub creator_affinity: f64,
    pub quality_ecpm: f64,
    pub freshness: f64,
    pub exploration_bonus: f64,
    pub mmr_penalty: f64,
    pub negative_penalty: f64,
    pub final_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecommendationExplanation {
    pub recall_channels: Vec<String>,
    pub primary_reason_zh: String,
    pub primary_reason_en: String,
    pub traffic_tier: String,
    pub breakdown: ScoreBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedSceneItemDto {
    pub asset: CodeSceneAssetDto,
    pub explanation: RecommendationExplanation,
}
```
- Additional DTOs: `CodeSceneFeedRequest` (`user_id`, `session_id`, `tab`, `tag_filter`, `query`, `cursor`, `limit`), `CodeSceneFeedResponse` (`items: Vec<RankedSceneItemDto>`, `next_cursor: Option<String>`, `total_catalog: usize`), `SceneBehaviorEventKind` (`Impression`, `Dwell`, `Complete`, `Replay`, `GameInteraction`, `Like`, `DoubleTapLike`, `Unlike`, `Bookmark`, `Unbookmark`, `Comment`, `Share`, `FollowAuthor`, `UnfollowAuthor`, `InspectCode`, `Remix`, `FastSkip`, `NotInterested`, `HideAuthor`, `HideTag`), `SceneBehaviorEventDto`, `CodeSceneCommentDto`, `PublishCodeSceneRequest/Response`, `AiGenerateSceneRequest/Response`, `AiIterateSceneRequest`, `RemixSceneRequest`, `ValidationReportDto`.

### 2. `gpui_telegram_code_scene::runtime` ↔ `gpui_telegram_desktop` & `gpui_telegram_server`
- `compile_and_verify(source: &str, manifest: &AssetManifest) -> Result<VerifiedProgram, CompileDiagnostic>` (parses, compiles, runs `on_init` + 5 frames `on_update`/`on_render`, verifies `>= 1` visible draw/pixel op, returns deterministic `cover_hash`).
- `VmInstance::new(program, budgets) -> Result<Self, RuntimeError>`
  - `step_frame(&mut self, dt_sec: f64, input: &SceneInputState) -> Result<FrameOutput, RuntimeError>`
  - `seek_video(&mut self, target_sec: f64) -> Result<FrameOutput, RuntimeError>`
  - `restart(&mut self) -> Result<(), RuntimeError>`
  - `set_playback_speed(&mut self, speed: f64)` (`0.5..=2.0`)
- `FrameOutput`: `{ draw_cmds: Vec<DrawCmd>, pixel_bgra: Option<PixelFrame>, timeline_sec: f64, duration_sec: Option<f64>, loop_count: u32, game_hud: Option<GameHudSnapshot>, audio_pulses: Vec<AudioPulse>, ops_used: u64 }`.
- Hard fail-closed `VmBudgets`: `max_source_bytes = 65_536`, `max_ast_depth = 64`, `max_bytecode_ops = 16_384`, `max_ops_per_frame = 100_000`, `max_frame_wall_ms = 10`, `max_call_depth = 48`, `max_heap_values = 32_768`, `max_draw_cmds = 2_048`, `max_pixel_area = 240 * 320`.

### 3. `gpui_telegram_code_scene::reco` ↔ `gpui_telegram_server` & `gpui_telegram_client`
- `CodeSceneRecommender` engine:
  - `recommend_feed(&self, req: &CodeSceneFeedRequest) -> CodeSceneFeedResponse`
  - `record_behavior(&mut self, event: &SceneBehaviorEventDto) -> RewardOutcome`
  - `register_or_update_asset(&mut self, asset: CodeSceneAssetDto, just_published_by_user: Option<i64>)`
  - `export_user_profile(&self, user_id: i64) -> Option<UserInterestProfile>` / `import_user_profile(&mut self, profile: UserInterestProfile)`
- `FeedSessionController`: manages active index, `[i-1, i, i+1]` preload window, live unseen-tail re-ranking after each interaction, and swipe-away dwell/completion/fast-skip telemetry emission.

### 4. `gpui_telegram_server` REST API ↔ `gpui_telegram_client` & E2E Tests
All routes authenticated via existing Bearer token middleware under `/api/v1/code-scene`:
- `POST /api/v1/code-scene/feed` -> `CodeSceneFeedResponse`
- `GET /api/v1/code-scene/assets` -> list/search assets
- `GET /api/v1/code-scene/assets/{id}` -> `CodeSceneAssetDto`
- `POST /api/v1/code-scene/assets` -> publish validated scene (`201`/`200` + `PublishCodeSceneResponse`; `422` on validation failure)
- `POST /api/v1/code-scene/assets/validate` -> `ValidationReportDto`
- `POST /api/v1/code-scene/assets/{id}/like` -> toggle like (`{ liked, likes_count }`)
- `POST /api/v1/code-scene/assets/{id}/bookmark` -> toggle bookmark (`{ bookmarked, bookmarks_count }`)
- `GET /api/v1/code-scene/assets/{id}/comments` & `POST /api/v1/code-scene/assets/{id}/comments`
- `POST /api/v1/code-scene/creators/{id}/follow` -> toggle follow
- `POST /api/v1/code-scene/telemetry` -> batch/single `SceneBehaviorEventDto` -> `{ accepted, reward, profile_updated }`
- `POST /api/v1/code-scene/ai/generate` -> generate + validate draft (`AiGenerateSceneResponse`)
- `POST /api/v1/code-scene/ai/iterate` -> refine draft with prompt
- `POST /api/v1/code-scene/ai/publish` -> atomically validate, persist, register in recommender, optionally post `GenerativeCard` to `chat_id`
- `POST /api/v1/code-scene/ai/remix` -> remix existing `scene_id` with prompt and publish/draft

### 5. Chat ↔ Code Scene Callbacks (`gpui_telegram_server` / `gpui_telegram_desktop`)
- Chat inline button / `GenerativeCard` `action_payload` prefixes:
  - `code_scene:open:<scene_id>` -> opens `ActiveWorkspacePanel::CodeScene` directly focused on `<scene_id>`
  - `code_scene:remix:<scene_id>` -> opens Code Scene AI Bot Studio pre-loaded with `<scene_id>` source code for remixing
  - `code_scene:share:<scene_id>` -> opens `ModalDialogKind::ForwardMessages` to forward a `GenerativeCard` for `<scene_id>` to any chat

---

## Code Layout

- `crates/gpui_telegram_proto/src/code_scene_contracts.rs` — Shared wire & domain DTOs
- `crates/gpui_telegram_code_scene/` — New pure-Rust workspace crate (added to root `Cargo.toml` members):
  - `src/lib.rs`, `src/model.rs`
  - `src/lang/{mod,lexer,parser,ast,compiler,vm,stdlib,limits}.rs`
  - `src/runtime/{mod,frame,pixel,session,timeline,validator}.rs`
  - `src/seed_library.rs`
  - `src/reco/{mod,embedding,telemetry,profile,recall,scorer,reranker,engine,eval}.rs`
  - `src/feed_session.rs`
  - `src/synth/{mod,intent,procedural,llm_prompt}.rs`
- `crates/gpui_telegram_agent/src/code_scene_bot.rs` + `hermes/{toolsets,commands}.rs` — AI Bot creation tools, `/scene`, `ProviderRouter` + self-repair + offline fallback
- `crates/gpui_telegram_server/src/db/code_scene_store.rs` (`MIGRATION_V42_CODE_SCENE`), `db/migrations.rs`, `routes/code_scene.rs`, `routes/mod.rs`, `server.rs`, `hermes_im_bridge.rs`
- `crates/gpui_telegram_client/src/net/code_scene_client.rs`, `src/cache/sqlite_cache.rs`, `src/cache/local_store.rs`, `src/code_scene_service.rs`
- `crates/gpui_telegram_desktop/assets/icons/code_scene.svg`
- `crates/gpui_telegram_desktop/src/theme/assets_embedded.rs` (`IconName::EXTRA` ONLY!)
- `crates/gpui_telegram_desktop/src/views/code_scene/{mod,viewport,gesture,hud_overlay,drawers,workspace}.rs`
- `crates/gpui_telegram_desktop/src/views/app_shell/shell_code_scene.rs` + integration hooks in `nav_rail.rs`, `sidebar/*`, `shell.rs`, `shell_{shortcuts,observers,input_ime,theme,stage_actions,miniapps_bots,overlays_render}.rs`, `semantic_tree.rs`, `main_menu_drawer.rs`
- E2E Testing Track (exclusive ownership, zero collision with repo's historical `TEST_INFRA.md`/`TEST_READY.md`):
  - `.agents/teamwork/e2e_orch_code_scene/CODE_SCENE_TEST_INFRA.md`
  - `.agents/teamwork/e2e_orch_code_scene/CODE_SCENE_TEST_READY.md`
  - `tests/e2e_suite/tests/code_scene_tier{1_feature,2_boundary,3_cross_feature,4_real_world}_tests.rs`
  - `tests/e2e/test_code_scene_e2e.py` (opaque-box HTTP/server/AI-bot end-to-end runner)

---

## Repository Invariants & Build Hygiene (NON-NEGOTIABLE)

1. **`AGENTS.md`**: Keep `debug = 0`, `strip = "debuginfo"`, `incremental = false` in `[profile.dev]`, `[profile.test]`, `[profile.ci]`; single `target/` directory; run `python3 scripts/clean_build_garbage.py` immediately after EVERY `cargo build`/`check`/`test`; use `tempfile::TempDir` for all test DBs/fixtures.
2. **Hard Desktop Invariants**:
   - `IconName::ALL.len() == 64` -> add `CodeScene` icon strictly to `IconName::EXTRA` (`assets_embedded.rs:124-126`).
   - `NavRail::new().items.len()` -> update `tests/desktop_views_tests.rs:593` (`10 -> 11`) in lockstep in M5; `unread_badge == 0` for `CodeScene`.
   - `ChatSidebar::new()` / `configure_for_cloud_workspace` MUST keep `dialogs.is_empty()` (`fusion_sidebar_truth_tests.rs:15-95`).
   - Preserve `milestone_1_nav_calls_regression_tests.rs:703-764` source invariants and `shell.rs:1781` `"0" => None` (use `Cmd/Ctrl+Shift+S` for Code Scene).
   - `gpui_telegram_desktop` sets `[lib] test = false` -> put pure logic tests in `gpui_telegram_code_scene` and desktop/GPUI headless tests in `crates/gpui_telegram_desktop/tests/` using `Application::headless()` subprocess harness.
   - Always call `cx.drop_image(prev, None)` on every `RenderImage` swap (`cdp_surface.rs:360-362`).
3. **Ledger Closure (`scripts/verify_ledger_closure.py`)**: Zero `#![allow(dead_code)]` / `#![allow(unused)]` shims; preserve all 3,441 ledger files and symbols; pass `cargo check --workspace --lib --bins` and `python3 scripts/verify_ledger_closure.py --ci-without-upstream --skip-cargo`.
