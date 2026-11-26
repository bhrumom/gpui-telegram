#![forbid(unsafe_code)]
//! Pure-Rust sandboxed `CodeSceneScript` (`cs1`) VM, multi-modal runtime
//! (`Video` / `Animation` / `Game`), pre-publish dry-run validator, and
//! 18-asset real-code seed library for Code Scene ("代码场景").

pub mod feed_session;
pub mod lang;
pub mod model;
pub mod reco;
pub mod runtime;
pub mod seed_library;
pub mod seeds;
pub mod synth;

pub use feed_session::{
    COMPLETE_RATIO_THRESHOLD, DEFAULT_ANIM_REF_DURATION_MS, DEFAULT_GAME_REF_DURATION_MS,
    DEFAULT_PREFETCH_THRESHOLD, FAST_SKIP_THRESHOLD_MS, FeedSessionController,
};
pub use lang::{CompiledProgram, VmBudgets, compile_source};
pub use model::{
    AudioPulse, CompileDiagnostic, DiagnosticStage, GameHudSnapshot, GameSessionTelemetry,
    RgbaColor, RuntimeError, SceneInputState, Span, ToneWave,
};
pub use reco::{
    CodeSceneRecommender, EMBEDDING_DIM, RecommenderConfig, RewardOutcome, SeenEntry,
    UserInterestProfile,
};
pub use runtime::{
    DrawCmd, FrameOutput, MAX_PIXEL_AREA, MAX_PIXEL_HEIGHT, MAX_PIXEL_WIDTH, PixelBuffer,
    PixelFrame, TextAlign, VerifiedProgram, VmInstance, compile_and_verify,
    compile_and_verify_with_budgets, compute_cover_hash, validate_to_report,
};
pub use seed_library::{
    OFFICIAL_ARCADE_FORGE_BOT_ID, OFFICIAL_CINEMA_CODE_BOT_ID, OFFICIAL_CODE_SCENE_BOTS,
    OFFICIAL_FABUSHI_BOT_ID, OFFICIAL_NEON_SHADER_BOT_ID, OFFICIAL_PHYSICS_LAB_BOT_ID,
    OfficialCodeSceneBot, find_official_bot, find_seed_by_id, official_seed_assets,
};
pub use synth::{
    ColorPaletteTheme, DeterministicProceduralSynthesizer, ENGINE_PROCEDURAL_OFFLINE,
    ENGINE_PROCEDURAL_REMIX, FEW_SHOT_ANIMATION_CS1, FEW_SHOT_GAME_CS1, FEW_SHOT_VIDEO_CS1,
    IntentAnalyzer, LlmParseError, LlmPromptBuilder, LlmPromptMessages, LlmResponseParser,
    PaletteSpec, ParsedLlmScene, RemixModifiers, SceneArchetype, SceneIntentSpec, SynthRequest,
    SynthesizedDraft,
};
