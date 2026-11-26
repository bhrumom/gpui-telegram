use openbot_contracts::agents::{
    AGENT_BACKUP_MODEL_LIMIT, AgentAccess, AgentBackupModels, AgentModelChoice,
    AgentReasoningEffort, AgentSummary, AgentWelcomeInput, LegacyAgentReasoningEffort,
    UpdateAgentInput, agent_task_limit, agent_welcome_message, folder_inside,
    is_browser_profile_id, legacy_reasoning_effort,
};
use openbot_contracts::avatar::{
    AVATAR_COLOR_CHOICES, AVATAR_EXPRESSIONS, AVATAR_HUES, AgentAvatarMoodsInput,
    AvatarColorPresetId, AvatarHue, AvatarMimeType, AvatarMood, AvatarStyle, BloubAvatarProfile,
    BloubExpression, BloubShape, SUPPORTED_AVATAR_SILHOUETTES, avatar_candidate_seeds,
    avatar_default_seed, avatar_file_extension, avatar_seed_for_face, avatar_style_for_seed,
    bloub_avatar_profile, compute_agent_avatar_moods, fnv1a_utf16_32, is_valid_avatar_image,
    nearest_avatar_hue, server_plush_hex, working_cycle_for_seed,
};
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_contracts::providers::{
    ACP_AGENT_PRESETS, AGENT_PROVIDER_DESCRIPTORS, AGENT_PROVIDERS, AgentProviderId,
    AgentProviderRef, CustomProviderEndpoint, CustomProviderHarness, CustomProviderHeader,
    CustomProviderInputKind, CustomProviderModel, CustomProviderReasoningEffort,
    DEFAULT_MODEL_SERVERS, ForeignProviderId, PICKER_PROVIDERS, composed_custom_model_id,
    custom_provider_endpoint_key, default_provider_model, encoded_model_id,
    is_custom_provider_inputs, is_custom_provider_reasoning_efforts, provider_for_legacy_model,
    saved_custom_model_id,
};

#[test]
fn fnv1a_utf16_and_bloub_avatar_parity() {
    // Exact FNV-1a 32-bit UTF-16 vectors (no "openbot" fallback on "")
    assert_eq!(fnv1a_utf16_32(""), 0x811c_9dc5);
    assert_eq!(utf16_len("🤖"), 2);
    assert_eq!(utf16_len("小龙虾"), 3);
    assert_eq!(utf16_len("Renée"), 5);

    // Verify surrogate pair UTF-16 code units hash identically to manual UTF-16 loop
    let emoji = "🦞✨";
    let mut manual = 0x811c_9dc5u32;
    for unit in emoji.encode_utf16() {
        manual = (manual ^ u32::from(unit)).wrapping_mul(0x0100_0193);
    }
    assert_eq!(fnv1a_utf16_32(emoji), manual);

    assert_eq!(SUPPORTED_AVATAR_SILHOUETTES.len(), 13);
    assert_eq!(AVATAR_EXPRESSIONS.len(), 16);
    assert_eq!(AVATAR_HUES.len(), 10);
    assert_eq!(AVATAR_COLOR_CHOICES.len(), 17);

    // Mine and verify "goutte" bucket-7 remap seeds + CJK + accented + emoji seeds
    let mut goutte_remapped = 0usize;
    for i in 0..200 {
        let seed = format!("seed-{i}");
        let shape_idx = (fnv1a_utf16_32(&format!("{seed}:shape")) as usize) % 11;
        let profile = BloubAvatarProfile::resolve(&seed, None, None);
        assert_ne!(profile.shape.as_str(), "goutte");
        if shape_idx == 7 {
            goutte_remapped += 1;
            let repl_idx = (fnv1a_utf16_32(&format!("{seed}:shape:replacement")) as usize) % 10;
            let expected = [
                BloubShape::Cercle,
                BloubShape::Galet,
                BloubShape::Squircle,
                BloubShape::Capsule,
                BloubShape::Triangle,
                BloubShape::Hexagone,
                BloubShape::Nuage,
                BloubShape::Carre,
                BloubShape::Losange,
                BloubShape::Pentagone,
            ][repl_idx];
            assert_eq!(profile.shape, expected);
        }
    }
    assert!(goutte_remapped >= 10, "expected multiple goutte bucket-7 hits");

    // Named silhouette overrides (fantome, meduse, poulpe) and plush vs flat style
    for named in [BloubShape::Fantome, BloubShape::Meduse, BloubShape::Poulpe] {
        let plush_seed =
            avatar_seed_for_face("agent-1", named, BloubExpression::Curieux, AvatarStyle::Plush)
                .expect("plush seed found");
        assert_eq!(avatar_style_for_seed(&plush_seed), AvatarStyle::Plush);
        let resolved = bloub_avatar_profile(&plush_seed, Some(AvatarHue::Blue), None);
        assert_eq!(resolved.shape, named);
        assert_eq!(resolved.expression, BloubExpression::Curieux);
        assert_eq!(resolved.color, "#3b93f0");

        let flat_seed =
            avatar_seed_for_face("agent-1", named, BloubExpression::Heureux, AvatarStyle::Flat)
                .expect("flat seed found");
        assert_eq!(avatar_style_for_seed(&flat_seed), AvatarStyle::Flat);
    }

    let default_flat = avatar_default_seed("agent-42", AvatarStyle::Flat).expect("flat seed");
    let p_plush = bloub_avatar_profile("agent-42", None, None);
    let p_flat = bloub_avatar_profile(&default_flat, None, None);
    assert_eq!(p_plush.shape, p_flat.shape);
    assert_eq!(p_plush.expression, p_flat.expression);

    // Candidate batch produces 12 seeds covering all 10 seeded silhouettes
    let cands = avatar_candidate_seeds("agent-42", "agent-42", 1);
    assert_eq!(cands.len(), 12);
    assert_eq!(cands[0], "agent-42");

    // AvatarHue serializes as JSON integer u16 (NOT string)
    let hue_json = serde_json::to_string(&AvatarHue::Blue).unwrap();
    assert_eq!(hue_json, "215");
    assert_eq!(serde_json::from_str::<AvatarHue>("215").unwrap(), AvatarHue::Blue);
    assert!(serde_json::from_str::<AvatarHue>("45").is_err());
    assert!(serde_json::from_str::<AvatarHue>("\"215\"").is_err());

    // nearest_avatar_hue neutral saturation < 0.15 -> None & preset hue snaps
    assert_eq!(nearest_avatar_hue("#a3a3a3"), None);
    assert_eq!(nearest_avatar_hue("#0a0a0c"), None);
    assert_eq!(nearest_avatar_hue("#7a7875"), None);
    assert_eq!(nearest_avatar_hue("#f1efe9"), Some(AvatarHue::Yellow));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Pink.hex()), Some(AvatarHue::Magenta));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Sky.hex()), Some(AvatarHue::Cyan));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Navy.hex()), Some(AvatarHue::Blue));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Wine.hex()), Some(AvatarHue::Red));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Brown.hex()), Some(AvatarHue::Orange));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Sand.hex()), Some(AvatarHue::Orange));
    assert_eq!(nearest_avatar_hue(AvatarColorPresetId::Grey.hex()), None);
    assert_eq!(server_plush_hex("srv-1", Some("#AABBCC")), "#aabbcc");

    // Mood priority & working cycle non-repetition & image magic bytes
    let mut mood_in = AgentAvatarMoodsInput {
        agent_ids: vec!["a1".into(), "a2".into(), "a3".into(), "a4".into()],
        ..Default::default()
    };
    mood_in.failed_turns.insert("a1".into());
    mood_in.pending_approvals.insert("a1".into());
    mood_in.pending_prompts.insert("a2".into());
    mood_in.working_agents.insert("a2".into());
    mood_in.working_agents.insert("a3".into());
    mood_in.recent_replies.insert("a4".into());
    let moods = compute_agent_avatar_moods(&mood_in);
    assert_eq!(moods.get("a1"), Some(&AvatarMood::Failed));
    assert_eq!(moods.get("a2"), Some(&AvatarMood::Waiting));
    assert_eq!(moods.get("a3"), Some(&AvatarMood::Working));
    assert_eq!(moods.get("a4"), Some(&AvatarMood::Responded));

    let mut prev = working_cycle_for_seed("bot-x", 0);
    for step in 1..20 {
        let next = working_cycle_for_seed("bot-x", step);
        assert_ne!(prev, next);
        prev = next;
    }

    assert!(is_valid_avatar_image("image/png", &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]));
    assert!(is_valid_avatar_image("image/jpeg", &[0xff, 0xd8, 0xff, 0xe0]));
    assert!(is_valid_avatar_image("image/webp", b"RIFF\x00\x00\x00\x00WEBPvp8 "));
    assert!(!is_valid_avatar_image("image/png", b"not-a-png"));
    assert_eq!(avatar_file_extension(AvatarMimeType::Jpeg), "jpg");
}

#[test]
fn providers_and_agents_contracts_roundtrip() {
    assert_eq!(AGENT_PROVIDERS.len(), 18);
    assert_eq!(AGENT_PROVIDER_DESCRIPTORS.len(), 18);
    assert_eq!(PICKER_PROVIDERS.len(), 17);
    assert_eq!(PICKER_PROVIDERS[0], AgentProviderId::Claude);
    assert_eq!(PICKER_PROVIDERS[1], AgentProviderId::Codex);
    assert_eq!(default_provider_model(AgentProviderId::Codex), "gpt-6-luna");
    assert_eq!(default_provider_model(AgentProviderId::Claude), "claude-haiku-5-5");
    assert_eq!(default_provider_model(AgentProviderId::Grok), "grok-4.7");
    assert_eq!(default_provider_model(AgentProviderId::Inception), "mercury-3");
    assert_eq!(default_provider_model(AgentProviderId::DeepSeek), "deepseek-v4-pro");
    assert_eq!(default_provider_model(AgentProviderId::OpenRouter), "anthropic/claude-sonnet-5.5");
    assert_eq!(provider_for_legacy_model("claude-3-opus"), AgentProviderId::Claude);
    assert_eq!(provider_for_legacy_model("grok-2"), AgentProviderId::Grok);
    assert_eq!(provider_for_legacy_model("o3-mini"), AgentProviderId::Codex);
    assert_eq!(agent_task_limit(AgentProviderId::Acp, Some(4), 4), 1);
    assert_eq!(agent_task_limit(AgentProviderId::Codex, Some(3), 4), 3);
    assert_eq!(ACP_AGENT_PRESETS.len(), 3);
    assert_eq!(DEFAULT_MODEL_SERVERS.len(), 2);

    assert!(ForeignProviderId::parse("codex").is_none());
    let fp = ForeignProviderId::parse("future-provider-9").expect("valid foreign provider");
    let pref: AgentProviderRef = serde_json::from_str("\"future-provider-9\"").unwrap();
    assert_eq!(pref, AgentProviderRef::Foreign(fp));

    assert_eq!(composed_custom_model_id("ep1", "org/model@v1+fast"), "ep1/org/model=40v1=2Bfast");
    assert_eq!(encoded_model_id("ep1/", "b64:raw"), "ep1/b64:YjY0OnJhdw");
    assert_eq!(saved_custom_model_id("ep1", Some(CustomProviderHarness::Pi), "m!1"), "ep1/b64:bSEx");
    assert!(is_custom_provider_inputs(&[CustomProviderInputKind::Image, CustomProviderInputKind::Pdf]));
    assert!(!is_custom_provider_inputs(&[CustomProviderInputKind::Pdf, CustomProviderInputKind::Image]));
    assert!(is_custom_provider_reasoning_efforts(&[CustomProviderReasoningEffort::Low, CustomProviderReasoningEffort::High]));
    assert!(!is_custom_provider_reasoning_efforts(&[CustomProviderReasoningEffort::High, CustomProviderReasoningEffort::Low]));
    assert_eq!(
        custom_provider_endpoint_key("http://localhost:80/v1/"),
        Some("http://127.0.0.1/v1".to_string())
    );

    // 39-field AgentSummary, tri-state UpdateAgentInput, BackupModels <= 8, reasoning efforts
    assert_eq!(legacy_reasoning_effort(AgentReasoningEffort::None), LegacyAgentReasoningEffort::Low);
    assert_eq!(legacy_reasoning_effort(AgentReasoningEffort::Ultra), LegacyAgentReasoningEffort::Max);

    let mut summary = AgentSummary::new_default("agent-1", "Atlas", "/tmp/ws");
    summary.access = Some(AgentAccess::Workspace);
    summary.avatar_hue = Some(AvatarHue::Indigo);
    summary.backup_models = Some(AgentBackupModels {
        automatic: true,
        order: vec![AgentModelChoice {
            provider: AgentProviderId::Claude,
            model: "claude-haiku-5-5".into(),
            provider_account_id: None,
        }],
    });
    assert!(summary.validate());
    assert!(summary.validate_strict_wire());
    let json = serde_json::to_string(&summary).unwrap();
    let decoded: AgentSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, summary);

    // > 8 backup models rejected
    let mut bad = summary.clone();
    bad.backup_models = Some(AgentBackupModels {
        automatic: true,
        order: (0..=AGENT_BACKUP_MODEL_LIMIT)
            .map(|i| AgentModelChoice {
                provider: AgentProviderId::Codex,
                model: format!("gpt-6-{i}"),
                provider_account_id: None,
            })
            .collect(),
    });
    assert!(!bad.validate());

    // Tri-state UpdateAgentInput: omitted vs explicit null vs value
    let patch: UpdateAgentInput =
        serde_json::from_str(r#"{"agentId":"agent-1","avatarHue":null,"taskLimit":4}"#).unwrap();
    assert_eq!(patch.allow_automation, None);
    assert_eq!(patch.avatar_hue, Some(None));
    assert_eq!(patch.task_limit, Some(Some(4)));
    assert_eq!(INPUT_LIMITS.message_text, 100_000);
    assert!(agent_welcome_message(&AgentWelcomeInput { language: "English".into(), first_request_follows: false }).contains("openbot.ask_user"));
    assert!(agent_welcome_message(&AgentWelcomeInput { language: "English".into(), first_request_follows: true }).contains("next message"));

    // 32-hex browserProfileId guard & folder_inside single-slash strip
    assert!(is_browser_profile_id("0123456789abcdef0123456789abcdef"));
    assert!(!is_browser_profile_id("profile-not-32-hex"));
    assert!(!is_browser_profile_id("0123456789ABCDEF0123456789abcdef"));
    let mut with_profile = summary.clone();
    with_profile.browser_profile_id = Some("0123456789abcdef0123456789abcdef".into());
    assert!(with_profile.validate());
    assert!(with_profile.validate_strict_wire());
    with_profile.browser_profile_id = Some("profile-not-32-hex".into());
    assert!(!with_profile.validate());
    assert!(!with_profile.validate_strict_wire());

    assert!(!folder_inside("//", "/tmp/ws"));
    assert!(folder_inside("/", "/tmp/ws"));
    assert!(folder_inside("/tmp/ws/", "/tmp/ws/sub"));
    assert!(!folder_inside("/tmp/ws/", "/tmp/ws"));

    // CustomProviderEndpoint::is_valid URL, models, and duplicate header checks
    let valid_ep = CustomProviderEndpoint {
        id: "my-llm".into(),
        name: "My LLM".into(),
        base_url: "https://api.example.com/v1".into(),
        api_key: Some("sk-test".into()),
        models: vec![CustomProviderModel {
            id: "gpt-custom".into(),
            name: "GPT Custom".into(),
            context_window: Some(128_000),
            max_output: Some(8_192),
            reasoning_efforts: None,
            inputs: None,
        }],
        headers: vec![CustomProviderHeader {
            name: "X-Custom-Org".into(),
            value: "acme".into(),
        }],
        harness: None,
    };
    assert!(valid_ep.is_valid());

    let mut ftp_ep = valid_ep.clone();
    ftp_ep.base_url = "ftp://api.example.com/v1".into();
    assert!(!ftp_ep.is_valid());

    let mut cred_ep = valid_ep.clone();
    cred_ep.base_url = "https://user:pass@api.example.com/v1".into();
    assert!(!cred_ep.is_valid());

    let mut empty_models_ep = valid_ep.clone();
    empty_models_ep.models.clear();
    assert!(!empty_models_ep.is_valid());

    let mut dup_models_ep = valid_ep.clone();
    dup_models_ep.models.push(valid_ep.models[0].clone());
    assert!(!dup_models_ep.is_valid());

    let mut bad_model_chars_ep = valid_ep.clone();
    bad_model_chars_ep.models[0].id = "has space".into();
    assert!(!bad_model_chars_ep.is_valid());

    let mut dup_headers_ep = valid_ep.clone();
    dup_headers_ep.headers.push(CustomProviderHeader {
        name: "x-custom-org".into(),
        value: "other".into(),
    });
    assert!(!dup_headers_ep.is_valid());
}

#[test]
fn iter4_agents_providers_and_limits_strict_wire_parity() {
    use openbot_contracts::agents::{
        AgentDefaults, AgentProfileDraft, AgentTask, AgentTaskOrigin, CreateAgentInput,
        GenerateAgentProfileInput, HiddenAgent, HiddenAgentsSnapshot, SaveAgentProfileInput,
        SetAgentAvatarInput,
    };
    use openbot_contracts::limits::is_uuid_v4;
    use openbot_contracts::providers::{
        CustomAgentCheckResult, CustomAgentEnvInput, CustomAgentSummary, CustomProviderHeaderInput,
        CustomProviderSummary, DetectedAcpAgent, DetectedModel, DetectedModelServer,
        SaveCustomAgentInput, UpdateCustomProviderInput,
    };

    // RFC4122 v4 UUID variant nibbles [89ab] accepted; non-RFC4122 nibbles rejected
    assert!(is_uuid_v4("550e8400-e29b-41d4-a716-446655440000"));
    assert!(is_uuid_v4("550e8400-e29b-41d4-8716-446655440000"));
    assert!(!is_uuid_v4("550e8400-e29b-41d4-0716-446655440000"));
    assert!(!is_uuid_v4("550e8400-e29b-41d4-c716-446655440000"));

    // AgentSummary: required nullable keys vs optional non-nullable keys + forward-compatible extra keys
    let base_agent = serde_json::to_value(AgentSummary::new_default("a1", "Atlas", "/ws")).unwrap();
    for req_null_key in ["threadId", "updatedAt", "avatarHue", "avatarUrl"] {
        let mut missing = base_agent.clone();
        missing.as_object_mut().unwrap().remove(req_null_key);
        assert!(
            serde_json::from_value::<AgentSummary>(missing).is_err(),
            "AgentSummary must reject omitted {req_null_key}"
        );
    }
    for opt_non_null_key in [
        "chatColor",
        "computerUse",
        "memory",
        "access",
        "taskLimit",
        "backupModels",
        "hiddenAt",
        "browserProfileId",
        "sessionSettingOverrides",
        "marketplaceSource",
    ] {
        let mut with_null = base_agent.clone();
        with_null.as_object_mut().unwrap().insert(opt_non_null_key.into(), serde_json::Value::Null);
        assert!(
            serde_json::from_value::<AgentSummary>(with_null).is_err(),
            "AgentSummary must reject explicit null on {opt_non_null_key}"
        );
    }
    let mut with_extra_and_null_color = base_agent;
    with_extra_and_null_color
        .as_object_mut()
        .unwrap()
        .insert("avatarColor".into(), serde_json::Value::Null);
    with_extra_and_null_color
        .as_object_mut()
        .unwrap()
        .insert("futureField".into(), serde_json::json!(true));
    let parsed_agent: AgentSummary = serde_json::from_value(with_extra_and_null_color).unwrap();
    assert_eq!(parsed_agent.avatar_color, Some(None));

    // UpdateAgentInput: rejects null on 14 `?: T` fields, rejects `"role"`, allows benign unknown keys
    for key in [
        "name",
        "title",
        "description",
        "notifications",
        "provider",
        "model",
        "reasoningEffort",
        "access",
        "computerUse",
        "memory",
        "fastMode",
        "ultrafast",
        "leaveUsageLimit",
        "avatarSeed",
    ] {
        let bad = serde_json::json!({ "agentId": "a1", key: null });
        assert!(
            serde_json::from_value::<UpdateAgentInput>(bad).is_err(),
            "UpdateAgentInput must reject null on {key}"
        );
    }
    assert!(serde_json::from_str::<UpdateAgentInput>(r#"{"agentId":"a1","role":"lead"}"#).is_err());
    assert!(serde_json::from_str::<UpdateAgentInput>(r#"{"agentId":"a1","role":null}"#).is_err());
    let ok_upd: UpdateAgentInput =
        serde_json::from_str(r#"{"agentId":"a1","benignExtra":123,"chatColor":null}"#).unwrap();
    assert_eq!(ok_upd.chat_color, Some(None));
    assert!(ok_upd.is_valid());

    // CreateAgentInput, SetAgentAvatarInput, AgentProfileDraft, Generate/SaveAgentProfileInput, AgentDefaults
    assert!(serde_json::from_str::<CreateAgentInput>(
        r#"{"name":"A","description":"","avatarSeed":"a1","initialMessage":"hi"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<CreateAgentInput>(
        r#"{"name":"A","description":"","avatarSeed":"a1","avatarHue":null,"initialMessage":"hi","provider":null}"#
    )
    .is_err());
    assert!(serde_json::from_str::<SetAgentAvatarInput>(r#"{"agentId":"a1"}"#).is_err());
    assert!(serde_json::from_str::<AgentDefaults>(
        r#"{"allowAutomation":false,"allowNewChat":false,"quickAcknowledgement":false,"automationRunsPerHour":30,"commitCoAuthor":true}"#
    )
    .is_err());
    assert!(serde_json::from_str::<AgentProfileDraft>(
        r#"{"name":"A","title":"","description":"D","avatarSeed":"a1","avatarHue":null}"#
    )
    .is_err());
    assert!(serde_json::from_str::<GenerateAgentProfileInput>(r#"{"prompt":"hi","agentId":null}"#).is_err());
    assert!(serde_json::from_str::<SaveAgentProfileInput>(
        r#"{"operationId":"550e8400-e29b-41d4-a716-446655440000","draft":{"name":"A","title":"","description":"D","avatarSeed":"a1","avatarHue":null,"sectionId":null},"initialMessage":null}"#
    )
    .is_err());

    // HiddenAgent rejects unknown keys; HiddenAgentsSnapshot allows unknown keys
    assert!(serde_json::from_str::<HiddenAgent>(
        r#"{"agentId":"a1","hiddenAt":"2026-11-25T00:00:00Z","extra":1}"#
    )
    .is_err());
    let snap: HiddenAgentsSnapshot = serde_json::from_str(
        r#"{"agents":[{"agentId":"a1","hiddenAt":"2026-11-25T00:00:00Z"}],"extra":1}"#,
    )
    .unwrap();
    assert!(snap.is_valid());

    // AgentTask: constrained origin, required nullables, double_option branch/routineId, non-null queue/awaitedReplies
    let task_ok: AgentTask = serde_json::from_value(serde_json::json!({
        "id": "t1",
        "agentId": "a1",
        "threadId": "th1",
        "origin": "agent",
        "title": "Side task",
        "state": "running",
        "outcome": null,
        "hold": null,
        "activeTurnId": "turn-1",
        "progress": null,
        "error": null,
        "queuedMessages": [],
        "awaitedReplies": [{"messageId": "m1", "recipientAgentId": "a2", "status": "running"}],
        "createdAt": "2026-11-25T00:00:00Z",
        "updatedAt": "2026-11-25T00:00:00Z",
        "closedAt": null,
        "branch": null,
        "routineId": null
    }))
    .unwrap();
    assert_eq!(task_ok.origin, AgentTaskOrigin::Agent);
    assert_eq!(task_ok.branch, Some(None));
    assert_eq!(task_ok.routine_id, Some(None));
    assert!(task_ok.is_valid());

    let mut bad_origin = serde_json::to_value(&task_ok).unwrap();
    bad_origin["origin"] = serde_json::json!("invalid");
    assert!(serde_json::from_value::<AgentTask>(bad_origin).is_err());

    let mut null_queue = serde_json::to_value(&task_ok).unwrap();
    null_queue["queue"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<AgentTask>(null_queue).is_err());

    // Providers: UpdateCustomProviderInput, forbidden secret keys, & DetectedModel::is_valid
    assert!(serde_json::from_str::<UpdateCustomProviderInput>(
        r#"{"id":"p1","name":"P","baseUrl":"http://127.0.0.1:8080","models":[],"apiKey":null}"#
    )
    .is_err());
    assert!(serde_json::from_str::<CustomProviderSummary>(
        r#"{"id":"p1","name":"P","baseUrl":"http://127.0.0.1:8080","hasApiKey":true,"models":[],"apiKey":"leak"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<DetectedModelServer>(
        r#"{"id":"ollama","name":"Ollama","baseUrl":"http://127.0.0.1:11434/v1","models":[],"headers":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<CustomAgentSummary>(
        r#"{"id":"my-acp","name":"ACP","command":"acp","args":[],"envNames":[],"resolvedCommand":null,"secret":"x"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<DetectedAcpAgent>(
        r#"{"id":"my-acp","name":"ACP","command":"acp","args":[],"env":{}}"#
    )
    .is_err());
    assert!(serde_json::from_str::<CustomProviderHeaderInput>(r#"{"name":"X-Test"}"#).is_err());
    assert!(serde_json::from_str::<CustomAgentEnvInput>(r#"{"name":"API_KEY"}"#).is_err());
    assert!(serde_json::from_str::<SaveCustomAgentInput>(
        r#"{"id":"my-acp","name":"ACP","command":"acp","args":[],"env":[]}"#
    )
    .is_err());
    assert!(serde_json::from_str::<CustomAgentCheckResult>(
        r#"{"agentName":"ACP","protocolVersion":1,"capabilities":[],"models":[]}"#
    )
    .is_err());

    let dm_missing_cw: DetectedModel =
        serde_json::from_str(r#"{"id":"m1","maxOutput":4096}"#).unwrap();
    assert!(!dm_missing_cw.is_valid());
}
