use std::collections::VecDeque;
use std::sync::Mutex;

use gpui_telegram_agent::hermes::commands::{
    SlashCommandParser, SlashCommandRegistry, StandaloneCommandHost,
};
use gpui_telegram_agent::hermes::providers::{
    ApiMode, CredentialPool, ProviderConfig, ProviderRouter,
};
use gpui_telegram_agent::hermes::toolsets::{
    all_builtin_tool_specs, HermesToolContext, HermesToolExecutor, ToolsetId,
};
use gpui_telegram_agent::{
    build_code_scene_generative_card, detect_code_scene_chat_intent, parse_scene_slash_args,
    CodeSceneBotGenerator, CodeSceneBoxFuture, CodeSceneChatIntent, CodeSceneLlmBackend,
    SceneSlashAction,
};
use gpui_telegram_code_scene::{
    compile_and_verify, LlmPromptMessages, ENGINE_PROCEDURAL_OFFLINE, ENGINE_PROCEDURAL_REMIX,
};
use gpui_telegram_proto::cards::GenerativeComponent;
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneRequest, AssetKind, AssetManifest, CodeSceneAssetDto, CodeSceneCardAction,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct ScriptedMockLlm {
    responses: Mutex<VecDeque<Result<(String, String), String>>>,
    seen_prompts: Mutex<Vec<LlmPromptMessages>>,
}

impl ScriptedMockLlm {
    fn new(items: Vec<Result<(String, String), String>>) -> Self {
        Self {
            responses: Mutex::new(items.into()),
            seen_prompts: Mutex::new(Vec::new()),
        }
    }

    fn prompts(&self) -> Vec<LlmPromptMessages> {
        self.seen_prompts.lock().unwrap().clone()
    }
}

impl CodeSceneLlmBackend for ScriptedMockLlm {
    fn complete_scene_prompt<'a>(
        &'a self,
        msgs: LlmPromptMessages,
    ) -> CodeSceneBoxFuture<'a, Result<(String, String), String>> {
        Box::pin(async move {
            self.seen_prompts.lock().unwrap().push(msgs);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("No scripted response left".to_string()))
        })
    }
}

fn valid_game_json(title: &str) -> String {
    let cs1 = r#"
fn on_init(state) {
    state.score = 0;
    state.x = 180;
    state.vx = 120;
}
fn on_update(dt, input, state) {
    state.x = state.x + state.vx * dt;
    if state.x > 320 || state.x < 40 {
        state.vx = -state.vx;
    }
    if input.pressed {
        state.score = state.score + 1;
    }
}
fn on_render(canvas, state, time) {
    clear(rgba(10, 12, 26, 1.0));
    circle(state.x, 320, 26, rgba(0, 240, 255, 0.95));
    text(20, 36, "SCORE " + str(state.score), 16, rgba(255, 255, 255, 0.95), "left");
}
"#;
    json!({
        "title": title,
        "description": "LLM generated neon arcade scene",
        "kind": "game",
        "tags": ["neon", "arcade"],
        "controls_hint": "Tap or press Space",
        "manifest": {
            "kind": "game",
            "width": 360,
            "height": 640,
            "fps": 60,
            "seed": 7
        },
        "source_code": cs1
    })
    .to_string()
}

#[tokio::test]
async fn test_code_scene_bot_generator_0_1_2_repairs_and_fallback() {
    let generator = CodeSceneBotGenerator::default();
    let req = AiGenerateSceneRequest {
        prompt: "Make a neon arcade pinball game".to_string(),
        kind: Some(AssetKind::Game),
        ..Default::default()
    };

    // Case 0: valid on first attempt
    let mock0 = ScriptedMockLlm::new(vec![Ok((
        valid_game_json("Neon Zero Repair"),
        "llm:openai:gpt-4o".to_string(),
    ))]);
    let out0 = generator.generate_with_backend(Some(&mock0), &req).await;
    assert!(!out0.used_fallback);
    assert_eq!(out0.repair_attempts, 0);
    assert!(out0.draft.validation.ok);
    assert_eq!(out0.draft.engine_used, "llm:openai:gpt-4o");
    assert_eq!(out0.draft.title, "Neon Zero Repair");

    // Case 1: syntax error on attempt 0 -> succeeds on repair attempt 1
    let bad_syntax = json!({
        "title": "Broken Syntax",
        "kind": "game",
        "source_code": "const x = 1; fn on_render(c, s, t) { clear(rgba(0,0,0,1)); }"
    })
    .to_string();
    let mock1 = ScriptedMockLlm::new(vec![
        Ok((bad_syntax, "llm:openai:gpt-4o".to_string())),
        Ok((
            valid_game_json("Repaired Attempt 1"),
            "llm:openai:gpt-4o".to_string(),
        )),
    ]);
    let out1 = generator.generate_with_backend(Some(&mock1), &req).await;
    assert!(!out1.used_fallback);
    assert_eq!(out1.repair_attempts, 1);
    assert!(out1.draft.validation.ok);
    assert_eq!(out1.draft.title, "Repaired Attempt 1");
    assert_eq!(mock1.prompts().len(), 2);
    assert!(mock1.prompts()[1].user.contains("Self-repair attempt 1"));

    // Case 2: missing on_render on attempt 0, empty draw on attempt 1 -> succeeds on attempt 2
    let no_render = json!({
        "title": "No Render",
        "kind": "game",
        "source_code": "fn on_init(state) { state.a = 1; }"
    })
    .to_string();
    let empty_draw = json!({
        "title": "Empty Draw",
        "kind": "game",
        "source_code": "fn on_render(canvas, state, time) { let x = 1 + 2; }"
    })
    .to_string();
    let mock2 = ScriptedMockLlm::new(vec![
        Ok((no_render, "llm:anthropic:claude".to_string())),
        Ok((empty_draw, "llm:anthropic:claude".to_string())),
        Ok((
            valid_game_json("Repaired Attempt 2"),
            "llm:anthropic:claude".to_string(),
        )),
    ]);
    let out2 = generator.generate_with_backend(Some(&mock2), &req).await;
    assert!(!out2.used_fallback);
    assert_eq!(out2.repair_attempts, 2);
    assert!(out2.draft.validation.ok);
    assert_eq!(out2.draft.title, "Repaired Attempt 2");
    assert_eq!(mock2.prompts().len(), 3);

    // Case 3: 3 consecutive failures -> deterministic offline fallback
    let mock3 = ScriptedMockLlm::new(vec![
        Ok(("not code at all".to_string(), "llm:openai:gpt-4o".to_string())),
        Ok(("still invalid".to_string(), "llm:openai:gpt-4o".to_string())),
        Ok((
            json!({ "source_code": "fn on_render(c, s, t) { let z = 1 / 0; }" }).to_string(),
            "llm:openai:gpt-4o".to_string(),
        )),
    ]);
    let out3 = generator.generate_with_backend(Some(&mock3), &req).await;
    assert!(out3.used_fallback);
    assert_eq!(out3.repair_attempts, 2);
    assert!(out3.draft.validation.ok);
    assert_eq!(out3.draft.engine_used, ENGINE_PROCEDURAL_OFFLINE);
    assert!(compile_and_verify(&out3.draft.source_code, &out3.draft.manifest).is_ok());

    // Remix fallback verification
    let remix_out = generator
        .iterate_or_remix_with_backend(
            None,
            &out0.draft.source_code,
            &out0.draft.manifest,
            Some(&out0.draft.title),
            &out0.draft.tags,
            "改成更快更炫的赛博霓虹配色",
        )
        .await;
    assert!(remix_out.used_fallback);
    assert!(remix_out.draft.validation.ok);
    assert_eq!(remix_out.draft.engine_used, ENGINE_PROCEDURAL_REMIX);
}

#[tokio::test]
async fn test_code_scene_provider_router_live_http_and_local_hermes_fallback() {
    // 1. Default ProviderRouter (LocalHermes) -> deterministic offline synthesis without HTTP.
    let local_router = ProviderRouter::new();
    let generator = CodeSceneBotGenerator::default();
    let req = AiGenerateSceneRequest {
        prompt: "赛博霓虹粒子流体动画".to_string(),
        kind: Some(AssetKind::Animation),
        ..Default::default()
    };
    let offline_out = generator.generate(Some(&local_router), &req).await;
    assert!(offline_out.used_fallback);
    assert!(offline_out.draft.validation.ok);
    assert_eq!(offline_out.draft.engine_used, ENGINE_PROCEDURAL_OFFLINE);

    // 2. Real loopback HTTP OpenAI-compatible server via ProviderRouter::complete
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let scene_payload = valid_game_json("Loopback Live LLM Game");
    let server_task = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 32768];
            let _ = socket.read(&mut buf).await;
            let body = json!({
                "id": "chatcmpl-test-cs1",
                "object": "chat.completion",
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": scene_payload
                    },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 120, "completion_tokens": 200, "total_tokens": 320 }
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });

    let live_router = ProviderRouter::new();
    let mut cfg = ProviderConfig::default();
    cfg.id = "mock_openai".to_string();
    cfg.name = "Mock OpenAI".to_string();
    cfg.api_mode = ApiMode::ChatCompletions;
    cfg.base_url = format!("http://{addr}/v1");
    cfg.default_model = "gpt-4o-mini".to_string();
    cfg.enabled = true;
    cfg.no_key_required = true;
    cfg.credentials = CredentialPool::default();
    live_router.register_provider(cfg);
    live_router
        .set_active("mock_openai", Some("gpt-4o-mini"))
        .unwrap();

    let live_req = AiGenerateSceneRequest {
        prompt: "Make an arcade bounce game".to_string(),
        kind: Some(AssetKind::Game),
        ..Default::default()
    };
    let live_out = generator.generate(Some(&live_router), &live_req).await;
    let _ = server_task.await;
    if std::env::var("GPUI_OFFLINE_TEST").ok().as_deref() != Some("1") {
        assert!(!live_out.used_fallback);
        assert_eq!(live_out.repair_attempts, 0);
        assert!(live_out.draft.validation.ok);
        assert!(live_out.draft.engine_used.starts_with("llm:mock_openai:"));
        assert_eq!(live_out.draft.title, "Loopback Live LLM Game");
    } else {
        assert!(live_out.draft.validation.ok);
    }
}

#[tokio::test]
async fn test_code_scene_toolsets_29_invariant_tir_safety_and_standalone_validate() {
    // Hard invariant: keep ToolsetId::all().len() == 29!
    assert_eq!(ToolsetId::all().len(), 29);

    let specs = all_builtin_tool_specs();
    let expected_tools = [
        "code_scene_create_draft",
        "code_scene_iterate_draft",
        "code_scene_validate",
        "code_scene_publish",
        "code_scene_remix",
        "code_scene_list_mine",
    ];
    for name in expected_tools {
        let spec = specs
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("Missing tool spec {name}"));
        assert_eq!(spec.toolset, ToolsetId::PlatformIm);
        // Critical TIR PATH_KEYS safety: NEVER expose a parameter named "source"
        if let Some(props) = spec
            .parameters_schema
            .get("properties")
            .and_then(|v| v.as_object())
        {
            assert!(
                !props.contains_key("source"),
                "Tool {name} must use 'source_code', never 'source' (TIR PATH_KEYS trap)"
            );
        }
    }

    // Verify TIR passes cs1 source_code containing `0..10` ranges and `//` comments
    // AND code_scene_validate works standalone without a PlatformImBridge!
    let tmp = tempfile::tempdir().unwrap();
    let ctx = HermesToolContext::new(tmp.path());
    let cs1_with_ranges_and_comments = r#"
// Neon bars demo with range loop 0..10
fn on_render(canvas, state, time) {
    clear(rgba(8, 10, 22, 1.0));
    for i in 0..10 {
        rect(20 + i * 32, 200, 24, 180, hsla(180 + i * 14, 0.9, 0.6, 0.9));
    }
}
"#;
    let res = HermesToolExecutor::execute(
        "code_scene_validate",
        json!({
            "source_code": cs1_with_ranges_and_comments,
            "kind": "animation"
        }),
        &ctx,
    )
    .await;
    assert!(
        res.success,
        "code_scene_validate must pass TIR and validate cleanly: {:?}",
        res.error
    );
    let structured = res.structured.expect("structured validation report");
    assert_eq!(structured.get("ok").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn test_code_scene_slash_command_aliases_and_intent_matrix() {
    let reg = SlashCommandRegistry::new();
    assert!(reg.get("scene").is_some());
    assert!(reg.get("codescene").is_some());
    assert!(reg.get("create-scene").is_some());
    assert!(!SlashCommandParser::is_bot_config_command("scene"));

    let mut host = StandaloneCommandHost::default();
    let parsed_help = SlashCommandParser::parse("/scene").unwrap();
    let out_help = reg.execute(&parsed_help, &mut host);
    assert!(out_help.handled && !out_help.is_error && !out_help.should_continue_to_llm);

    let parsed_create =
        SlashCommandParser::parse("/scene@fabushi_ai_bot create game 霓虹弹球小游戏").unwrap();
    assert_eq!(parsed_create.bot_mention.as_deref(), Some("fabushi_ai_bot"));
    let out_create = reg.execute(&parsed_create, &mut host);
    assert!(out_create.should_continue_to_llm);
    let rewritten = out_create.rewritten_prompt.as_deref().unwrap();
    assert!(rewritten.contains("`code_scene_publish`"));
    assert_eq!(
        detect_code_scene_chat_intent(rewritten),
        Some(CodeSceneChatIntent::CreateAndPublish {
            prompt: "霓虹弹球小游戏".to_string(),
            kind: Some(AssetKind::Game),
        })
    );

    let parsed_remix =
        SlashCommandParser::parse("/codescene remix seed_game_flappy_bird 改成赛博朋克风格")
            .unwrap();
    let out_remix = reg.execute(&parsed_remix, &mut host);
    assert!(out_remix.should_continue_to_llm);
    let remix_rewritten = out_remix.rewritten_prompt.as_deref().unwrap();
    assert_eq!(
        detect_code_scene_chat_intent(remix_rewritten),
        Some(CodeSceneChatIntent::Remix {
            scene_id: "seed_game_flappy_bird".to_string(),
            prompt: "改成赛博朋克风格".to_string(),
            auto_publish: true,
        })
    );

    assert_eq!(parse_scene_slash_args("mine"), SceneSlashAction::ListMine);

    // Positive natural language intents
    assert!(matches!(
        detect_code_scene_chat_intent("@fabushi_ai_bot 帮我做一个霓虹弹球小游戏发到代码场景"),
        Some(CodeSceneChatIntent::CreateAndPublish {
            kind: Some(AssetKind::Game),
            ..
        })
    ));
    assert!(matches!(
        detect_code_scene_chat_intent("帮我做一个赛博流体动画发到代码场景"),
        Some(CodeSceneChatIntent::CreateAndPublish {
            kind: Some(AssetKind::Animation),
            ..
        })
    ));
    assert!(matches!(
        detect_code_scene_chat_intent("create a neon pinball game for code scene"),
        Some(CodeSceneChatIntent::CreateAndPublish {
            kind: Some(AssetKind::Game),
            ..
        })
    ));
    assert_eq!(
        detect_code_scene_chat_intent("查看我在代码场景的作品"),
        Some(CodeSceneChatIntent::ListMine)
    );

    // Negative exclusions MUST return None
    for neg in [
        "你好",
        "打个视频通话吧",
        "今天玩游戏了吗",
        "什么是代码场景？",
        "代码场景怎么用",
        "在电脑上运行 ls",
        "打开法布施小程序",
    ] {
        assert_eq!(
            detect_code_scene_chat_intent(neg),
            None,
            "False positive on negative prompt: {neg}"
        );
    }

    // Canonical GenerativeCard verification
    let mut asset = CodeSceneAssetDto::default();
    asset.scene_id = "scene_abc123".to_string();
    asset.title = "霓虹弹球".to_string();
    asset.description = "炫彩弹球挑战".to_string();
    asset.kind = AssetKind::Game;
    asset.author_user_id = 42;
    asset.author_username = "alice".to_string();
    asset.creator_bot_id = Some(777_000);
    asset.creator_bot_username = Some("fabushi_ai_bot".to_string());
    asset.tags = vec!["neon".to_string(), "game".to_string()];
    asset.manifest = AssetManifest::for_kind(AssetKind::Game);

    let card = build_code_scene_generative_card(&asset);
    assert!(!card.card_id.is_empty());
    assert_eq!(card.title, "霓虹弹球");
    let payloads: Vec<String> = card
        .components
        .iter()
        .filter_map(|c| match c {
            GenerativeComponent::Button { action_payload, .. } => Some(action_payload.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        payloads,
        vec![
            CodeSceneCardAction::Open("scene_abc123".into()).to_payload(),
            CodeSceneCardAction::Remix("scene_abc123".into()).to_payload(),
            CodeSceneCardAction::Share("scene_abc123".into()).to_payload(),
        ]
    );
}

#[test]
fn test_code_scene_generative_card_escapes_all_5_html_chars_and_preserves_payloads() {
    let mut asset = CodeSceneAssetDto::default();
    asset.scene_id = "scene_xss_99".to_string();
    asset.title = r#"<script>alert("x")</script> & 'single'"#.to_string();
    asset.description = r#"Desc <b>"quoted"</b> & 'apos'"#.to_string();
    asset.kind = AssetKind::Animation;
    asset.author_user_id = 77;
    asset.author_username = r#"a<"b">'c&d"#.to_string();
    asset.creator_bot_id = Some(777_000);
    asset.creator_bot_username = Some(r#"bot<"x">'y&z"#.to_string());
    asset.tags = vec![r#"neon<"tag">'&"#.to_string()];
    asset.controls_hint = r#"Tap "A" & 'B' <now>"#.to_string();
    asset.manifest = AssetManifest::for_kind(AssetKind::Animation);

    let card = build_code_scene_generative_card(&asset);

    assert_eq!(
        card.title,
        "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;single&#39;"
    );
    let subtitle = card.subtitle.as_deref().unwrap();
    for raw_ch in ['<', '>', '"', '\''] {
        assert!(
            !card.title.contains(raw_ch),
            "card.title must escape '{raw_ch}': {}",
            card.title
        );
        assert!(
            !subtitle.contains(raw_ch),
            "card.subtitle must escape '{raw_ch}': {subtitle}"
        );
    }
    assert!(subtitle.contains("&quot;") && subtitle.contains("&#39;") && subtitle.contains("&amp;"));

    let mut texts = Vec::new();
    let mut payloads = Vec::new();
    for comp in &card.components {
        match comp {
            GenerativeComponent::Text { text, .. } => texts.push(text.as_str()),
            GenerativeComponent::Button { action_payload, .. } => {
                payloads.push(action_payload.as_str())
            }
            _ => {}
        }
    }
    assert!(texts.len() >= 2, "expected description + meta text components");
    for t in &texts {
        for raw_ch in ['<', '>', '"', '\''] {
            assert!(!t.contains(raw_ch), "component text must escape '{raw_ch}': {t}");
        }
        assert!(t.contains("&quot;") && t.contains("&#39;") && t.contains("&amp;"));
    }

    assert_eq!(
        payloads,
        vec![
            "code_scene:open:scene_xss_99",
            "code_scene:remix:scene_xss_99",
            "code_scene:share:scene_xss_99",
        ]
    );
}

