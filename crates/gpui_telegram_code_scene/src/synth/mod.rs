//! Dual-Engine AI Code Scene Synthesizer (`CS-F08`).
//!
//! Combines an offline-first deterministic procedural `cs1` generator & source-level
//! remixer (`DeterministicProceduralSynthesizer`) with live LLM prompt, few-shot,
//! compiler-diagnostic self-repair, and resilient response parsing (`LlmPromptBuilder`,
//! `LlmResponseParser`).

pub mod intent;
pub mod llm_prompt;
pub mod procedural;
mod templates_anim;
mod templates_game;
mod templates_video;

use gpui_telegram_proto::{
    AiGenerateSceneRequest, AiGenerateSceneResponse, AssetKind, AssetManifest,
    ValidationReportDto,
};
use serde::{Deserialize, Serialize};

pub use intent::{
    ColorPaletteTheme, IntentAnalyzer, PaletteSpec, RemixModifiers, SceneArchetype,
    SceneIntentSpec,
};
pub use llm_prompt::{
    FEW_SHOT_ANIMATION_CS1, FEW_SHOT_GAME_CS1, FEW_SHOT_VIDEO_CS1, LlmParseError,
    LlmPromptBuilder, LlmPromptMessages, LlmResponseParser, ParsedLlmScene,
};
pub use procedural::DeterministicProceduralSynthesizer;

pub const ENGINE_PROCEDURAL_OFFLINE: &str = "procedural_cs1_v1";
pub const ENGINE_PROCEDURAL_REMIX: &str = "procedural_remix_v1";

/// Input request for `IntentAnalyzer` and `DeterministicProceduralSynthesizer`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthRequest {
    pub prompt: String,
    pub kind: Option<AssetKind>,
    pub style_tags: Vec<String>,
    pub seed: Option<u64>,
}

impl From<&AiGenerateSceneRequest> for SynthRequest {
    fn from(req: &AiGenerateSceneRequest) -> Self {
        Self {
            prompt: req.prompt.clone(),
            kind: req.kind,
            style_tags: req.style_tags.clone().unwrap_or_default(),
            seed: req.seed,
        }
    }
}

/// Complete synthesized and validated Code Scene draft ready for preview or publishing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthesizedDraft {
    pub title: String,
    pub description: String,
    pub kind: AssetKind,
    pub tags: Vec<String>,
    pub source_code: String,
    pub code_format: String,
    pub manifest: AssetManifest,
    pub controls_hint: String,
    pub archetype: SceneArchetype,
    pub palette: ColorPaletteTheme,
    pub validation: ValidationReportDto,
    pub engine_used: String,
}

impl SynthesizedDraft {
    /// Converts this draft into the wire `AiGenerateSceneResponse` DTO for M3 server/agent routes.
    pub fn into_ai_generate_response(
        self,
        draft_id: impl Into<String>,
        bot_id: Option<i64>,
        bot_username: Option<String>,
        repair_attempts: u32,
    ) -> AiGenerateSceneResponse {
        AiGenerateSceneResponse {
            draft_id: draft_id.into(),
            title: self.title,
            description: self.description,
            kind: self.kind,
            tags: self.tags,
            source_code: self.source_code,
            code_format: self.code_format,
            manifest: self.manifest,
            controls_hint: self.controls_hint,
            validation: self.validation,
            engine_used: self.engine_used,
            repair_attempts,
            bot_id,
            bot_username,
        }
    }
}
