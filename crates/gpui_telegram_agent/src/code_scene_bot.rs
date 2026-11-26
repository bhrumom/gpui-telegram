//! AI Bot creation, self-repair, intent parsing, and GenerativeCard builders
//! for Code Scene ("代码场景", `CS-F10`).

use std::future::Future;
use std::pin::Pin;

use gpui_telegram_code_scene::{
    compile_and_verify_with_budgets, validate_to_report, DeterministicProceduralSynthesizer,
    LlmPromptBuilder, LlmPromptMessages, LlmResponseParser, SynthRequest, SynthesizedDraft,
    VmBudgets,
};
use gpui_telegram_proto::cards::{GenerativeCard, GenerativeCardLayout, GenerativeComponent};
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneRequest, AssetKind, AssetManifest, CodeSceneAssetDto, CodeSceneCardAction,
    ValidationDiagnosticDto, ValidationReportDto, ValidationSeverity,
};

use crate::hermes::providers::{
    ApiMode, ProviderChatMessage, ProviderRequest, ProviderRouter, SystemPromptTier,
};

pub type CodeSceneBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Pluggable LLM completion backend for `CodeSceneBotGenerator`.
pub trait CodeSceneLlmBackend: Send + Sync {
    /// Executes a Code Scene generation or self-repair prompt.
    /// Returns `Ok((raw_llm_content, engine_label))` on remote success, or `Err(reason)` when
    /// offline, `LocalHermes`, or upstream transport/auth fails.
    fn complete_scene_prompt<'a>(
        &'a self,
        msgs: LlmPromptMessages,
    ) -> CodeSceneBoxFuture<'a, Result<(String, String), String>>;
}

fn env_is_truthy(key: &str) -> bool {
    std::env::var(key).ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Checks whether a `ProviderRouter` currently has an eligible remote LLM target.
pub fn active_remote_target(router: &ProviderRouter) -> Option<(String, String, ApiMode)> {
    let (provider_id, active_model, api_mode) = router.active_info();
    if api_mode == ApiMode::LocalHermes
        || provider_id == "local_hermes"
        || provider_id.trim().is_empty()
    {
        return None;
    }
    if env_is_truthy("GPUI_OFFLINE_TEST") && !env_is_truthy("HERMES_FORCE_LIVE_PROVIDERS") {
        return None;
    }
    let snap = router.snapshot();
    let cfg = snap.providers.get(&provider_id)?;
    if !cfg.enabled {
        return None;
    }
    let usable =
        cfg.no_key_required || !cfg.credentials.is_empty() || !router.needs_provider_selection();
    if !usable {
        return None;
    }
    let model = if active_model.trim().is_empty() || active_model == "hermes-local-v1" {
        cfg.default_model.clone()
    } else {
        active_model
    };
    if model.trim().is_empty() {
        return None;
    }
    Some((provider_id, model, api_mode))
}

/// Live `ProviderRouter`-backed implementation of `CodeSceneLlmBackend`.
pub struct ProviderRouterBackend<'a> {
    router: &'a ProviderRouter,
}

impl<'a> ProviderRouterBackend<'a> {
    pub fn new(router: &'a ProviderRouter) -> Self {
        Self { router }
    }
}

impl<'a> CodeSceneLlmBackend for ProviderRouterBackend<'a> {
    fn complete_scene_prompt<'b>(
        &'b self,
        msgs: LlmPromptMessages,
    ) -> CodeSceneBoxFuture<'b, Result<(String, String), String>> {
        Box::pin(async move {
            let Some((_provider_id, model, _mode)) = active_remote_target(self.router) else {
                return Err("No active remote LLM provider configured".to_string());
            };
            let mut req = ProviderRequest::new(model, vec![ProviderChatMessage::user(msgs.user)]);
            req.system_tiers = vec![SystemPromptTier::stable("code_scene_cs1_spec", msgs.system)];
            req.stream = false;
            req.temperature = Some(0.35);
            req.max_tokens = Some(4096);

            let resp = self
                .router
                .complete(req, None)
                .await
                .map_err(|e| e.to_string())?;
            if resp.api_mode == ApiMode::LocalHermes {
                return Err("ProviderRouter fell back to LocalHermes".to_string());
            }
            let engine = format!("llm:{}:{}", resp.provider_id, resp.model);
            Ok((resp.content, engine))
        })
    }
}

/// Outcome of a Code Scene AI generation, iteration, or remix call.
#[derive(Debug, Clone)]
pub struct CodeSceneGenOutcome {
    pub draft: SynthesizedDraft,
    pub repair_attempts: u32,
    pub used_fallback: bool,
}

/// Dual-engine Code Scene generator: uses live `ProviderRouter` LLM with up to
/// `max_repair_attempts` compiler-diagnostic self-repair turns, falling back seamlessly
/// to the deterministic offline `DeterministicProceduralSynthesizer`.
#[derive(Debug, Clone)]
pub struct CodeSceneBotGenerator {
    pub max_repair_attempts: u32,
    pub force_offline: bool,
}

impl Default for CodeSceneBotGenerator {
    fn default() -> Self {
        Self {
            max_repair_attempts: 2,
            force_offline: false,
        }
    }
}

impl CodeSceneBotGenerator {
    pub fn offline() -> Self {
        Self {
            max_repair_attempts: 0,
            force_offline: true,
        }
    }

    pub fn validate(&self, source_code: &str, manifest: &AssetManifest) -> ValidationReportDto {
        validate_to_report(source_code, manifest)
    }

    pub async fn generate(
        &self,
        router: Option<&ProviderRouter>,
        req: &AiGenerateSceneRequest,
    ) -> CodeSceneGenOutcome {
        let backend = router
            .filter(|r| !self.force_offline && active_remote_target(r).is_some())
            .map(ProviderRouterBackend::new);
        let backend_ref: Option<&dyn CodeSceneLlmBackend> =
            backend.as_ref().map(|b| b as &dyn CodeSceneLlmBackend);
        self.generate_with_backend(backend_ref, req).await
    }

    pub async fn generate_with_backend(
        &self,
        backend: Option<&dyn CodeSceneLlmBackend>,
        req: &AiGenerateSceneRequest,
    ) -> CodeSceneGenOutcome {
        let synth_req = SynthRequest::from(req);
        let mut attempts_made: u32 = 0;

        if !self.force_offline {
            if let Some(llm) = backend {
                let init_msgs = LlmPromptBuilder::build_create_prompt(
                    &req.prompt,
                    req.kind,
                    &synth_req.style_tags,
                    req.seed,
                );
                if let Ok((raw, engine_label)) = llm.complete_scene_prompt(init_msgs).await {
                    match evaluate_llm_output(&raw, &req.prompt, req.kind, &engine_label) {
                        Ok(draft) => {
                            return CodeSceneGenOutcome {
                                draft,
                                repair_attempts: 0,
                                used_fallback: false,
                            };
                        }
                        Err((mut last_src, mut last_manifest, mut last_report)) => {
                            for attempt in 1..=self.max_repair_attempts {
                                attempts_made = attempt;
                                let repair_msgs = LlmPromptBuilder::build_repair_from_report(
                                    &req.prompt,
                                    &last_src,
                                    &last_manifest,
                                    &last_report,
                                    attempt,
                                );
                                match llm.complete_scene_prompt(repair_msgs).await {
                                    Ok((repair_raw, repair_engine)) => {
                                        match evaluate_llm_output(
                                            &repair_raw,
                                            &req.prompt,
                                            req.kind,
                                            &repair_engine,
                                        ) {
                                            Ok(draft) => {
                                                return CodeSceneGenOutcome {
                                                    draft,
                                                    repair_attempts: attempt,
                                                    used_fallback: false,
                                                };
                                            }
                                            Err((src, manifest, report)) => {
                                                last_src = src;
                                                last_manifest = manifest;
                                                last_report = report;
                                            }
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut draft = DeterministicProceduralSynthesizer::new().synthesize(&synth_req);
        revalidate_draft_if_wall_timeout(&mut draft);
        CodeSceneGenOutcome {
            draft,
            repair_attempts: attempts_made,
            used_fallback: true,
        }
    }

    pub async fn iterate_or_remix(
        &self,
        router: Option<&ProviderRouter>,
        existing_source: &str,
        existing_manifest: &AssetManifest,
        existing_title: Option<&str>,
        existing_tags: &[String],
        instruction: &str,
    ) -> CodeSceneGenOutcome {
        let backend = router
            .filter(|r| !self.force_offline && active_remote_target(r).is_some())
            .map(ProviderRouterBackend::new);
        let backend_ref: Option<&dyn CodeSceneLlmBackend> =
            backend.as_ref().map(|b| b as &dyn CodeSceneLlmBackend);
        self.iterate_or_remix_with_backend(
            backend_ref,
            existing_source,
            existing_manifest,
            existing_title,
            existing_tags,
            instruction,
        )
        .await
    }

    pub async fn iterate_or_remix_with_backend(
        &self,
        backend: Option<&dyn CodeSceneLlmBackend>,
        existing_source: &str,
        existing_manifest: &AssetManifest,
        existing_title: Option<&str>,
        existing_tags: &[String],
        instruction: &str,
    ) -> CodeSceneGenOutcome {
        let mut attempts_made: u32 = 0;
        let fallback_kind = Some(existing_manifest.kind);

        if !self.force_offline {
            if let Some(llm) = backend {
                let init_msgs = LlmPromptBuilder::build_remix_prompt(
                    existing_source,
                    existing_manifest,
                    existing_title,
                    instruction,
                );
                if let Ok((raw, engine_label)) = llm.complete_scene_prompt(init_msgs).await {
                    match evaluate_llm_output(&raw, instruction, fallback_kind, &engine_label) {
                        Ok(draft) => {
                            return CodeSceneGenOutcome {
                                draft,
                                repair_attempts: 0,
                                used_fallback: false,
                            };
                        }
                        Err((mut last_src, mut last_manifest, mut last_report)) => {
                            for attempt in 1..=self.max_repair_attempts {
                                attempts_made = attempt;
                                let repair_msgs = LlmPromptBuilder::build_repair_from_report(
                                    instruction,
                                    &last_src,
                                    &last_manifest,
                                    &last_report,
                                    attempt,
                                );
                                match llm.complete_scene_prompt(repair_msgs).await {
                                    Ok((repair_raw, repair_engine)) => {
                                        match evaluate_llm_output(
                                            &repair_raw,
                                            instruction,
                                            fallback_kind,
                                            &repair_engine,
                                        ) {
                                            Ok(draft) => {
                                                return CodeSceneGenOutcome {
                                                    draft,
                                                    repair_attempts: attempt,
                                                    used_fallback: false,
                                                };
                                            }
                                            Err((src, manifest, report)) => {
                                                last_src = src;
                                                last_manifest = manifest;
                                                last_report = report;
                                            }
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut draft = DeterministicProceduralSynthesizer::new().remix_source(
            existing_source,
            existing_manifest,
            instruction,
            existing_title,
            existing_tags,
        );
        revalidate_draft_if_wall_timeout(&mut draft);
        CodeSceneGenOutcome {
            draft,
            repair_attempts: attempts_made,
            used_fallback: true,
        }
    }
}

fn bot_validation_budgets() -> VmBudgets {
    #[cfg(debug_assertions)]
    {
        let mut budgets = VmBudgets::default();
        budgets.max_frame_wall_ms = 50;
        budgets
    }
    #[cfg(not(debug_assertions))]
    {
        VmBudgets::default()
    }
}

fn revalidate_draft_if_wall_timeout(draft: &mut SynthesizedDraft) {
    if draft.validation.ok {
        return;
    }
    let is_wall_timeout = draft.validation.diagnostics.iter().any(|d| {
        d.code == "E_DRY_RUN_FRAME" && d.message.contains("wall-clock timeout")
    });
    if !is_wall_timeout {
        return;
    }
    if let Ok(verified) =
        compile_and_verify_with_budgets(&draft.source_code, &draft.manifest, bot_validation_budgets())
    {
        let diagnostics: Vec<ValidationDiagnosticDto> = verified
            .warnings
            .iter()
            .map(|w| w.to_dto(ValidationSeverity::Warning))
            .collect();
        draft.validation = ValidationReportDto {
            ok: true,
            diagnostics,
            frames_executed: verified.draw_cmds_per_frame.len() as u32,
            draw_cmds_per_frame: verified.draw_cmds_per_frame,
            has_pixel_output: verified.has_pixel_output,
            peak_ops_used: verified.peak_ops_used,
            cover_hash: Some(verified.cover_hash),
            feature_tokens: verified.feature_tokens,
        };
    }
}

fn evaluate_llm_output(
    raw: &str,
    fallback_prompt: &str,
    fallback_kind: Option<AssetKind>,
    engine_label: &str,
) -> Result<SynthesizedDraft, (String, AssetManifest, ValidationReportDto)> {
    match LlmResponseParser::parse(raw) {
        Ok(parsed) => {
            let (mut draft, mut ok) =
                parsed.into_validated_draft(fallback_prompt, fallback_kind, engine_label);
            if !ok {
                revalidate_draft_if_wall_timeout(&mut draft);
                ok = draft.validation.ok;
            }
            if ok {
                Ok(draft)
            } else {
                Err((draft.source_code, draft.manifest, draft.validation))
            }
        }
        Err(parse_err) => {
            let truncated: String = raw.chars().take(4096).collect();
            let manifest = AssetManifest::for_kind(fallback_kind.unwrap_or(AssetKind::Animation));
            let report = ValidationReportDto {
                ok: false,
                diagnostics: vec![ValidationDiagnosticDto {
                    severity: ValidationSeverity::Error,
                    code: "E_PARSE_LLM_OUTPUT".to_string(),
                    message: parse_err.to_string(),
                    line: None,
                    column: None,
                    stage: Some("llm_parse".to_string()),
                }],
                frames_executed: 0,
                draw_cmds_per_frame: Vec::new(),
                has_pixel_output: false,
                peak_ops_used: 0,
                cover_hash: None,
                feature_tokens: Vec::new(),
            };
            Err((truncated, manifest, report))
        }
    }
}

/// Structured result of parsing `/scene` (and `/codescene`, `/create-scene`) arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneSlashAction {
    Help,
    ListMine,
    Create {
        kind: Option<AssetKind>,
        prompt: String,
    },
    Remix {
        scene_id: String,
        prompt: String,
    },
}

/// Detected natural-language or slash-command intent to invoke Code Scene tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeSceneChatIntent {
    CreateAndPublish {
        prompt: String,
        kind: Option<AssetKind>,
    },
    CreateDraft {
        prompt: String,
        kind: Option<AssetKind>,
    },
    Remix {
        scene_id: String,
        prompt: String,
        auto_publish: bool,
    },
    ListMine,
}

/// Parses arguments following `/scene`, `/codescene`, or `/create-scene`.
pub fn parse_scene_slash_args(args: &str) -> SceneSlashAction {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return SceneSlashAction::Help;
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().map(str::trim).unwrap_or("");
    let first_lower = first.to_ascii_lowercase();

    match first_lower.as_str() {
        "help" | "h" | "?" | "帮助" => return SceneSlashAction::Help,
        "mine" | "list" | "my" | "myworks" | "我的" | "我的作品" => {
            return SceneSlashAction::ListMine;
        }
        "remix" | "改编" | "二创" | "复刻" => {
            if rest.is_empty() {
                return SceneSlashAction::Help;
            }
            let mut rparts = rest.splitn(2, char::is_whitespace);
            let scene_id = rparts.next().unwrap_or("").trim().to_string();
            let remix_prompt = rparts.next().map(str::trim).unwrap_or("");
            if scene_id.is_empty() {
                return SceneSlashAction::Help;
            }
            let prompt = if remix_prompt.is_empty() {
                "创意改编升级".to_string()
            } else {
                remix_prompt.to_string()
            };
            return SceneSlashAction::Remix { scene_id, prompt };
        }
        _ => {}
    }

    let body = if matches!(
        first_lower.as_str(),
        "create" | "new" | "make" | "generate" | "gen" | "publish" | "创作" | "生成" | "创建"
    ) {
        rest
    } else {
        trimmed
    };
    if body.is_empty() {
        return SceneSlashAction::Help;
    }

    let mut bparts = body.splitn(2, char::is_whitespace);
    let maybe_kind_tok = bparts.next().unwrap_or("");
    let after_kind = bparts.next().map(str::trim).unwrap_or("");
    if let Some(kind) = AssetKind::parse(maybe_kind_tok) {
        if after_kind.is_empty() {
            return SceneSlashAction::Help;
        }
        return SceneSlashAction::Create {
            kind: Some(kind),
            prompt: after_kind.to_string(),
        };
    }

    SceneSlashAction::Create {
        kind: infer_kind_from_text(body),
        prompt: body.to_string(),
    }
}

fn infer_kind_from_text(text: &str) -> Option<AssetKind> {
    let lower = text.to_ascii_lowercase();
    if text.contains("小游戏")
        || text.contains("代码游戏")
        || text.contains("游戏")
        || text.contains("弹球")
        || text.contains("打砖块")
        || text.contains("贪吃蛇")
        || text.contains("像素鸟")
        || text.contains("太空射击")
        || text.contains("跑酷")
        || lower.contains("game")
        || lower.contains("pinball")
        || lower.contains("breakout")
        || lower.contains("arcade")
        || lower.contains("flappy")
        || lower.contains("snake")
        || lower.contains("shooter")
    {
        return Some(AssetKind::Game);
    }
    if text.contains("代码视频")
        || text.contains("短视频")
        || text.contains("视频")
        || text.contains("片头")
        || text.contains("星际")
        || lower.contains("video")
        || lower.contains("reel")
        || lower.contains("cinema")
        || lower.contains("timeline")
    {
        return Some(AssetKind::Video);
    }
    if text.contains("代码动画")
        || text.contains("动画")
        || text.contains("粒子")
        || text.contains("流体")
        || text.contains("着色器")
        || text.contains("分形")
        || lower.contains("animation")
        || lower.contains("shader")
        || lower.contains("plasma")
        || lower.contains("fractal")
        || lower.contains("particle")
    {
        return Some(AssetKind::Animation);
    }
    None
}

fn extract_scene_id_token(text: &str) -> Option<String> {
    for raw in text.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | '，' | '。' | ';' | '；' | ':' | '：' | '(' | ')' | '[' | ']' | '`' | '"' | '\'')
    }) {
        let tok = raw.trim();
        if tok.len() >= 5
            && (tok.starts_with("seed_") || tok.starts_with("scene_") || tok.starts_with("cs_"))
            && tok
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Some(tok.to_string());
        }
    }
    None
}

fn strip_leading_mentions(raw: &str) -> &str {
    let mut s = raw.trim();
    loop {
        if let Some(after_at) = s.strip_prefix('@') {
            if let Some(ws_idx) = after_at.find(char::is_whitespace) {
                let candidate = &after_at[..ws_idx];
                if !candidate.is_empty()
                    && candidate
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    s = after_at[ws_idx..].trim_start();
                    continue;
                }
            }
        }
        break;
    }
    s
}

fn extract_json_object_after<'a>(text: &'a str, marker: &str) -> Option<serde_json::Value> {
    let idx = text.find(marker)?;
    let rest = &text[idx + marker.len()..];
    let start = rest.find('{')?;
    let slice = &rest[start..];
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, ch) in slice.char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let candidate = &slice[..=i];
                    return serde_json::from_str::<serde_json::Value>(candidate).ok();
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_rewritten_tool_directive(text: &str) -> Option<CodeSceneChatIntent> {
    if text.contains("`code_scene_list_mine`") {
        return Some(CodeSceneChatIntent::ListMine);
    }
    if let Some(val) = extract_json_object_after(text, "`code_scene_publish`") {
        let prompt = val
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_string();
        let kind = val
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(AssetKind::parse);
        return Some(CodeSceneChatIntent::CreateAndPublish { prompt, kind });
    }
    if let Some(val) = extract_json_object_after(text, "`code_scene_create_draft`") {
        let prompt = val
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_string();
        let kind = val
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(AssetKind::parse);
        return Some(CodeSceneChatIntent::CreateDraft { prompt, kind });
    }
    if let Some(val) = extract_json_object_after(text, "`code_scene_remix`") {
        let scene_id = val
            .get("scene_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_string();
        let prompt = val
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("创意改编升级")
            .to_string();
        let auto_publish = val
            .get("auto_publish")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        return Some(CodeSceneChatIntent::Remix {
            scene_id,
            prompt,
            auto_publish,
        });
    }
    None
}

/// Detects whether a chat prompt (slash command, rewritten slash directive, or natural language)
/// intends to invoke a Code Scene creation/remix/list-mine workflow.
pub fn detect_code_scene_chat_intent(raw_text: &str) -> Option<CodeSceneChatIntent> {
    let cleaned = strip_leading_mentions(raw_text);
    if cleaned.is_empty() {
        return None;
    }

    // 1. Direct slash command `/scene`, `/codescene`, `/create-scene` (with optional `@bot`)
    if cleaned.starts_with('/') {
        let mut parts = cleaned.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let rest = parts.next().map(str::trim).unwrap_or("");
        let cmd_raw = head.trim_start_matches('/');
        let cmd_base = cmd_raw.split('@').next().unwrap_or("").to_ascii_lowercase();
        if matches!(cmd_base.as_str(), "scene" | "codescene" | "create-scene") {
            return match parse_scene_slash_args(rest) {
                SceneSlashAction::Help => None,
                SceneSlashAction::ListMine => Some(CodeSceneChatIntent::ListMine),
                SceneSlashAction::Create { kind, prompt } => {
                    Some(CodeSceneChatIntent::CreateAndPublish { prompt, kind })
                }
                SceneSlashAction::Remix { scene_id, prompt } => Some(CodeSceneChatIntent::Remix {
                    scene_id,
                    prompt,
                    auto_publish: true,
                }),
            };
        }
        return None;
    }

    // 2. Rewritten slash prompt emitted by `SlashCommandRegistry::execute` for `/scene`.
    if let Some(intent) = parse_rewritten_tool_directive(cleaned) {
        return Some(intent);
    }

    let lower = cleaned.to_ascii_lowercase();

    // 3. Negative exclusions: never hijack voice/video calls, MiniApps, or Remote Computer Use.
    let excluded = [
        "视频通话",
        "语音通话",
        "miniapp",
        "mini app",
        "小程序",
        "在电脑上",
        "远程执行",
        "远程电脑",
    ];
    if excluded.iter().any(|kw| cleaned.contains(kw) || lower.contains(kw)) {
        return None;
    }

    let wants_draft = cleaned.contains("仅草稿")
        || cleaned.contains("先存草稿")
        || cleaned.contains("生成草稿")
        || lower.contains("draft only")
        || lower.contains("as a draft");

    // 4. Explicit "代码场景" / "code scene" / "codescene" cue
    let has_code_scene_cue = cleaned.contains("代码场景")
        || lower.contains("code scene")
        || lower.contains("codescene")
        || lower.contains("code_scene");
    if has_code_scene_cue {
        let is_info_question = cleaned.contains("什么是代码场景")
            || cleaned.contains("代码场景是什么")
            || cleaned.contains("代码场景怎么用")
            || cleaned.contains("怎么用代码场景")
            || lower.contains("what is code scene")
            || lower.contains("how does code scene work");
        if is_info_question {
            return None;
        }

        let is_list_mine = (cleaned.contains("我的")
            || (cleaned.contains('我') && (cleaned.contains("作品") || cleaned.contains("发布")))
            || lower.contains("my works")
            || lower.contains("my scenes")
            || lower.contains("list mine"))
            && (cleaned.contains("查看")
                || cleaned.contains("列出")
                || cleaned.contains("有哪些")
                || cleaned.contains("作品")
                || lower.contains("list")
                || lower.contains("show"));
        if is_list_mine {
            return Some(CodeSceneChatIntent::ListMine);
        }

        let is_remix = cleaned.contains("改编")
            || cleaned.contains("二创")
            || cleaned.contains("复刻")
            || lower.contains("remix");
        if is_remix {
            if let Some(scene_id) = extract_scene_id_token(cleaned) {
                return Some(CodeSceneChatIntent::Remix {
                    scene_id,
                    prompt: cleaned.to_string(),
                    auto_publish: !wants_draft,
                });
            }
        }

        let has_action = [
            "做", "创作", "生成", "制作", "写", "发布", "发到", "创建", "打造", "设计",
            "create", "make", "build", "generate", "publish", "post", "design",
        ]
        .iter()
        .any(|kw| cleaned.contains(kw) || lower.contains(kw));
        let kind = infer_kind_from_text(cleaned);
        if has_action || kind.is_some() {
            return if wants_draft {
                Some(CodeSceneChatIntent::CreateDraft {
                    prompt: cleaned.to_string(),
                    kind,
                })
            } else {
                Some(CodeSceneChatIntent::CreateAndPublish {
                    prompt: cleaned.to_string(),
                    kind,
                })
            };
        }
        return None;
    }

    // 5. Implicit creation request: requires BOTH creation verb AND specific Code Scene artifact noun.
    let has_create_verb = [
        "帮我做", "给我做", "做一个", "做个", "创作一个", "生成一个", "写一个", "制作一个",
        "create a ", "make a ", "build a ", "generate a ",
    ]
    .iter()
    .any(|kw| cleaned.contains(kw) || lower.contains(kw));
    let has_artifact_noun = [
        "代码视频", "代码动画", "代码游戏", "弹球小游戏", "打砖块小游戏", "贪吃蛇小游戏",
        "像素鸟小游戏", "太空射击小游戏", "跑酷小游戏", "互动小游戏", "粒子动画", "流体动画",
        "着色器动画", "分形动画", "code video", "code animation", "code game", "pinball game",
        "breakout game", "space shooter", "snake game", "flappy bird", "plasma animation",
        "shader animation",
    ]
    .iter()
    .any(|kw| cleaned.contains(kw) || lower.contains(kw));

    if has_create_verb && has_artifact_noun {
        let kind = infer_kind_from_text(cleaned);
        return if wants_draft {
            Some(CodeSceneChatIntent::CreateDraft {
                prompt: cleaned.to_string(),
                kind,
            })
        } else {
            Some(CodeSceneChatIntent::CreateAndPublish {
                prompt: cleaned.to_string(),
                kind,
            })
        };
    }

    None
}

fn escape_card_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Canonical `GenerativeCard` builder for a published Code Scene asset.
/// Used uniformly by REST publish/remix endpoints, `ServerPlatformImBridge`, and direct bot turns.
pub fn build_code_scene_generative_card(asset: &CodeSceneAssetDto) -> GenerativeCard {
    let raw_title = if asset.title.trim().is_empty() {
        "Code Scene"
    } else {
        asset.title.trim()
    };
    let title = escape_card_text(raw_title);
    let bot_label = asset
        .creator_bot_username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|u| format!("🤖 @{}", escape_card_text(u.trim_start_matches('@'))));
    let author_label = if asset.author_username.trim().is_empty() {
        format!("@user_{}", asset.author_user_id)
    } else {
        format!("@{}", escape_card_text(asset.author_username.trim().trim_start_matches('@')))
    };
    let subtitle = match bot_label {
        Some(bot) => format!(
            "{} · {} · by {} · {}x{}@{}fps",
            asset.kind.label_zh(),
            bot,
            author_label,
            asset.manifest.width,
            asset.manifest.height,
            asset.manifest.fps
        ),
        None => format!(
            "{} · by {} · {}x{}@{}fps",
            asset.kind.label_zh(),
            author_label,
            asset.manifest.width,
            asset.manifest.height,
            asset.manifest.fps
        ),
    };
    let badge_color = match asset.kind {
        AssetKind::Video => "#7C3AED",
        AssetKind::Animation => "#06B6D4",
        AssetKind::Game => "#EC4899",
    };
    let description = if asset.description.trim().is_empty() {
        format!("{} ({})", title, asset.kind.label_en())
    } else {
        escape_card_text(asset.description.trim())
    };

    let mut components = vec![
        GenerativeComponent::Badge {
            label: format!("代码场景 · {} ({})", asset.kind.label_zh(), asset.kind.label_en()),
            color: badge_color.to_string(),
        },
        GenerativeComponent::Text {
            text: description,
            bold: false,
            color: None,
        },
    ];

    let mut meta_parts = Vec::new();
    if !asset.tags.is_empty() {
        let tags_line = asset
            .tags
            .iter()
            .take(6)
            .map(|t| format!("#{}", escape_card_text(t.trim().trim_start_matches('#'))))
            .collect::<Vec<_>>()
            .join(" ");
        if !tags_line.is_empty() {
            meta_parts.push(tags_line);
        }
    }
    if !asset.controls_hint.trim().is_empty() {
        meta_parts.push(escape_card_text(asset.controls_hint.trim()));
    }
    if !meta_parts.is_empty() {
        components.push(GenerativeComponent::Text {
            text: meta_parts.join(" · "),
            bold: false,
            color: Some("#94A3B8".to_string()),
        });
    }

    components.push(GenerativeComponent::Divider);
    components.push(GenerativeComponent::Button {
        id: "open".to_string(),
        label: "▶ 在代码场景中打开".to_string(),
        action_payload: CodeSceneCardAction::Open(asset.scene_id.clone()).to_payload(),
    });
    components.push(GenerativeComponent::Button {
        id: "remix".to_string(),
        label: "✨ Remix 改编".to_string(),
        action_payload: CodeSceneCardAction::Remix(asset.scene_id.clone()).to_payload(),
    });
    components.push(GenerativeComponent::Button {
        id: "share".to_string(),
        label: "↗ 转发分享".to_string(),
        action_payload: CodeSceneCardAction::Share(asset.scene_id.clone()).to_payload(),
    });

    GenerativeCard {
        card_id: format!("code_scene:{}", asset.scene_id),
        title,
        subtitle: Some(subtitle),
        layout: GenerativeCardLayout::VerticalStack,
        components,
        status: Some("published".to_string()),
    }
}
